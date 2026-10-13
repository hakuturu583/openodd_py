//! openodd's coverage view and case plot in the browser.
//!
//! A [`Viewer`] reads coverage documents (the contents of `*_coverage.json`
//! files, as one JSON array) and renders, client side, what the Python
//! package renders on a server: the ODD coverage matrix
//! ([`Viewer::coverage_html`], [`Viewer::coverage_json`]) and the case plot
//! with its predicted background ([`Viewer::cases_html`],
//! [`Viewer::cases_json`]). The background is openodd's own flaky model --
//! the same Gaussian-process classifier, defaults and seed as the merged
//! report -- fitted in the browser: once per selection of runs, and its grid
//! once per pair of axes, both cached.
//!
//! Every call takes its query and options as JSON strings, the same keys as
//! the Python API (`x`, `y`, `conditions`, `scenario`, `environment`, ...;
//! `id`, `link`, `labels`, `table`, `prediction`, `instability_rings`). The
//! HTML needs [`styles`] and [`script`] on the page once; `js/viewer.js`
//! (`mount`) re-renders a view when its axis pickers or cells are used.

use std::cell::RefCell;
use std::collections::HashMap;

use openodd::case_prediction::CasePredictor;
use openodd_coverage_view::cases::{Case, CaseQuery, CaseSet};
use openodd_coverage_view::prediction::Predictor;
use openodd_coverage_view::{case_plot, html, Dataset, Query};
use serde_json::{json, Value as Json};
use wasm_bindgen::prelude::*;

fn parse(raw: &str, what: &str) -> Result<Json, String> {
    if raw.trim().is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str(raw).map_err(|e| format!("{what}: {e}"))
}

fn labels(opts: &Json) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    if let Some(map) = opts.get("labels").and_then(Json::as_object) {
        for (k, v) in map {
            let v = v.as_str().ok_or(format!("label {k}: not a string"))?;
            out.push((k.clone(), v.to_string()));
        }
    }
    Ok(out)
}

fn text(opts: &Json, key: &str) -> Option<String> {
    opts.get(key).and_then(Json::as_str).map(String::from)
}

fn flag(opts: &Json, key: &str, default: bool) -> bool {
    opts.get(key).and_then(Json::as_bool).unwrap_or(default)
}

/// The coverage view's options from JSON: `{id, link, labels}`.
pub fn coverage_options(raw: &str) -> Result<html::Options, String> {
    let opts = parse(raw, "options")?;
    let mut out = html::Options {
        id: text(&opts, "id").unwrap_or_default(),
        link: text(&opts, "link"),
        ..Default::default()
    };
    for (k, v) in labels(&opts)? {
        if !out.labels.set(&k, &v) {
            return Err(format!("no such label: {k}"));
        }
    }
    Ok(out)
}

/// The case plot's options from JSON: `{id, link, labels, table,
/// instability_rings}`.
pub fn case_options(raw: &str) -> Result<case_plot::Options, String> {
    let opts = parse(raw, "options")?;
    let mut out = case_plot::Options {
        id: text(&opts, "id").unwrap_or_default(),
        link: text(&opts, "link"),
        table: flag(&opts, "table", true),
        instability_rings: flag(&opts, "instability_rings", false),
        drift_leaders: flag(&opts, "drift_leaders", false),
        ..Default::default()
    };
    if let Some(map) = opts.get("knobs").and_then(Json::as_object) {
        for (k, v) in map {
            let pair = v
                .as_array()
                .filter(|p| p.len() == 2)
                .and_then(|p| Some((p[0].as_f64()?, p[1].as_f64()?)))
                .ok_or(format!("knob {k}: [scale, offset]"))?;
            out.knobs.insert(k.clone(), pair);
        }
    }
    for (k, v) in labels(&opts)? {
        if !out.labels.set(&k, &v) {
            return Err(format!("no such label: {k}"));
        }
    }
    Ok(out)
}

/// A panic's message on the browser console (`console.error`), instead of
/// a bare `unreachable` trap: set once, when the module starts.
#[wasm_bindgen(start)]
pub fn start() {
    std::panic::set_hook(Box::new(|info| {
        console_error(&format!("openodd-wasm: {info}"))
    }));
}

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = console, js_name = error)]
    fn console_error(message: &str);
}

/// Coverage documents, read once, and the views of them.
#[wasm_bindgen]
pub struct Viewer {
    coverage: Dataset,
    cases: CaseSet,
    documents: Vec<Json>,
    /// The flaky models, by the selection of runs (scenario, environment,
    /// basis and its window):
    /// each keeps its grids (by pair, either way round, and detail) and the
    /// cases' pass probabilities.
    models: RefCell<HashMap<String, CasePredictor>>,
}

impl Viewer {
    /// Read the documents (a JSON array of coverage documents).
    pub fn from_json(documents_json: &str) -> Result<Viewer, String> {
        let raw: Json = serde_json::from_str(documents_json).map_err(|e| e.to_string())?;
        let documents = match raw {
            Json::Array(docs) => docs,
            one @ Json::Object(_) => vec![one],
            _ => return Err("expected a JSON array of coverage documents".into()),
        };
        Ok(Viewer {
            coverage: Dataset::from_documents(&documents),
            cases: CaseSet::from_documents(&documents),
            documents,
            models: RefCell::new(HashMap::new()),
        })
    }

    /// The model of a selection, fitted on first use and shared by
    /// `cases_html` and `cases_json`. As a [`Predictor`] its grids carry the
    /// pass probability only (what the colour needs: far cheaper);
    /// [`CasePredictor::grid_with`] gives the full metrics.
    fn with_model<T>(&self, query: &CaseQuery, f: impl FnOnce(&CasePredictor) -> T) -> T {
        let key = json!([
            query.scenario,
            query.environment,
            query.basis.as_str(),
            query.window()
        ])
        .to_string();
        let mut models = self.models.borrow_mut();
        let model = models
            .entry(key)
            .or_insert_with(|| CasePredictor::new(&self.cases, query).with_detail(false));
        f(model)
    }

    pub fn coverage_html_str(&self, query: &str, options: &str) -> Result<String, String> {
        let q = Query::from_json(&parse(query, "query")?)?;
        let m = self.coverage.matrix(&q);
        Ok(html::fragment(
            &self.coverage,
            &m,
            &coverage_options(options)?,
        ))
    }

    pub fn coverage_json_str(&self, query: &str) -> Result<String, String> {
        let q = Query::from_json(&parse(query, "query")?)?;
        Ok(self.coverage.matrix(&q).to_json().to_string())
    }

    pub fn cases_html_str(&self, query: &str, options: &str) -> Result<String, String> {
        let raw = parse(options, "options")?;
        let opts = case_options(options)?;
        let q = CaseQuery::from_json(&parse(query, "query")?)?;
        let plot = self.cases.plot(&q);
        if !flag(&raw, "prediction", true) {
            return Ok(case_plot::fragment(&self.cases, &plot, &opts));
        }
        Ok(self.with_model(&q, |m| {
            case_plot::fragment_with(&self.cases, &plot, &opts, Some(m as &dyn Predictor))
        }))
    }

    pub fn drift_html_str(&self, query: &str, options: &str) -> Result<String, String> {
        let q = CaseQuery::from_json(&parse(query, "query")?)?;
        Ok(case_plot::drift(&self.cases, &q, &case_options(options)?))
    }

    pub fn drift_json_str(&self, query: &str, options: &str) -> Result<String, String> {
        let q = CaseQuery::from_json(&parse(query, "query")?)?;
        let opts = case_options(options)?;
        Ok(self.cases.drift(&q).to_json_with(&opts.knobs).to_string())
    }

    pub fn cases_json_str(&self, query: &str, prediction: bool) -> Result<String, String> {
        let q = CaseQuery::from_json(&parse(query, "query")?)?;
        let plot = self.cases.plot(&q);
        let mut out = plot.to_json();
        if prediction {
            self.with_model(&q, |m| {
                out["prediction"] = match m.grid_with(&plot, true) {
                    Ok(g) => g.to_json(),
                    Err(why) => json!({"unavailable": why}),
                };
                let refs: Vec<&Case> = plot.cases.iter().collect();
                if let Some(cases) = out["cases"].as_array_mut() {
                    for (j, p) in cases.iter_mut().zip(m.pass_probabilities(&refs)) {
                        j["predicted_pass_probability"] = json!(p);
                    }
                }
            });
        }
        Ok(out.to_string())
    }
}

fn js(e: String) -> JsError {
    JsError::new(&e)
}

#[wasm_bindgen]
impl Viewer {
    /// Read coverage documents: a JSON array of `*_coverage.json` contents.
    #[wasm_bindgen(constructor)]
    pub fn new(documents_json: &str) -> Result<Viewer, JsError> {
        Viewer::from_json(documents_json).map_err(js)
    }

    /// Add more documents (a JSON array); the models are fitted again.
    pub fn add(&mut self, documents_json: &str) -> Result<(), JsError> {
        let more = Viewer::from_json(documents_json).map_err(js)?;
        self.documents.extend(more.documents);
        *self = Viewer {
            coverage: Dataset::from_documents(&self.documents),
            cases: CaseSet::from_documents(&self.documents),
            documents: std::mem::take(&mut self.documents),
            models: RefCell::new(HashMap::new()),
        };
        Ok(())
    }

    /// Documents read.
    #[wasm_bindgen(getter)]
    pub fn documents(&self) -> usize {
        self.documents.len()
    }

    /// The ODD attributes of the coverage matrix, as a JSON array.
    pub fn attributes(&self) -> String {
        json!(self.coverage.attributes()).to_string()
    }

    /// The attributes of the cases' space, as a JSON array.
    pub fn case_attributes(&self) -> String {
        json!(self.cases.attributes()).to_string()
    }

    /// The scenarios of the runs, as a JSON array.
    pub fn scenarios(&self) -> String {
        let mut all = self.coverage.scenarios();
        for s in self.cases.scenarios() {
            if !all.contains(&s) {
                all.push(s);
            }
        }
        all.sort();
        json!(all).to_string()
    }

    /// Distinct cases with a run that recorded an outcome.
    #[wasm_bindgen(getter)]
    pub fn cases(&self) -> usize {
        self.cases.cases.len()
    }

    /// The coverage matrix as an HTML fragment (host-driven: a click on a
    /// cell or a picker dispatches `openodd:drill` / `openodd:axes`).
    pub fn coverage_html(&self, query_json: &str, options_json: &str) -> Result<String, JsError> {
        self.coverage_html_str(query_json, options_json).map_err(js)
    }

    /// The coverage matrix as JSON.
    pub fn coverage_json(&self, query_json: &str) -> Result<String, JsError> {
        self.coverage_json_str(query_json).map_err(js)
    }

    /// The case plot as an HTML fragment, with the predicted background
    /// unless `options.prediction` is false.
    pub fn cases_html(&self, query_json: &str, options_json: &str) -> Result<String, JsError> {
        self.cases_html_str(query_json, options_json).map_err(js)
    }

    /// The scenario's realisation: drawn against measured, per attribute,
    /// with the knob calibration it suggests (`options.knobs`:
    /// `{attribute: [scale, offset]}` adds the corrected knob).
    pub fn drift_html(&self, query_json: &str, options_json: &str) -> Result<String, JsError> {
        self.drift_html_str(query_json, options_json).map_err(js)
    }

    /// The drift as JSON (per attribute: runs, summary, fit, calibration;
    /// with `options.knobs`, the suggested calibration's corrected knob).
    pub fn drift_json(&self, query_json: &str, options_json: &str) -> Result<String, JsError> {
        self.drift_json_str(query_json, options_json).map_err(js)
    }

    /// The case plot as JSON, with the prediction grid (`prediction`) and
    /// each case's `predicted_pass_probability` when `prediction` is true.
    pub fn cases_json(&self, query_json: &str, prediction: bool) -> Result<String, JsError> {
        self.cases_json_str(query_json, prediction).map_err(js)
    }
}

/// The components' CSS (coverage view and case plot): once per page.
#[wasm_bindgen]
pub fn styles() -> String {
    html::styles().to_string()
}

/// The components' script (events, tooltip cards): once per page.
#[wasm_bindgen]
pub fn script() -> String {
    html::script().to_string()
}
