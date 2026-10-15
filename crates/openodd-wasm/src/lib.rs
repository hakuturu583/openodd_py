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
//!
//! A [`Comparison`] reads two sets of documents measured on the same ODD --
//! a baseline and another set, say two versions of the software -- and
//! renders both views with the two laid over one another: the matrix's
//! cells split in two, the case plot's marks too, each outlined by how its
//! failure rate moved, and behind the case plot the change of the predicted
//! pass rate. It has the same methods as the [`Viewer`], so `mount` drives
//! it the same way.

use std::cell::RefCell;
use std::collections::HashMap;

use openodd::case_prediction::CasePredictor;
use openodd_coverage_view::case_plot::compare::Background;
use openodd_coverage_view::cases::{Case, CaseQuery, CaseSet};
use openodd_coverage_view::compare::{self, Side};
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

/// What a fitted model is cached by: the selection of runs (scenario,
/// environment, basis and its window). `environment` keeps "all" (`None`)
/// apart from "those that recorded none" (`Some(None)`).
fn model_key(query: &CaseQuery) -> String {
    json!([
        query.scenario,
        query.environment.is_some(),
        query.environment,
        query.basis.as_str(),
        query.window()
    ])
    .to_string()
}

/// Coverage documents from JSON: an array of them, or one.
fn documents(raw: &str) -> Result<Vec<Json>, String> {
    match serde_json::from_str(raw).map_err(|e| e.to_string())? {
        Json::Array(docs) => Ok(docs),
        one @ Json::Object(_) => Ok(vec![one]),
        _ => Err("expected a JSON array of coverage documents".into()),
    }
}

/// The comparison's words from the options' `labels` (those it has; the
/// rest are the component's) and `names` (`[baseline, other]`).
fn compare_words(opts: &Json, names: &[String; 2]) -> Result<compare::Labels, String> {
    let mut words = compare::Labels {
        base: names[0].clone(),
        other: names[1].clone(),
        ..Default::default()
    };
    if let Some(n) = opts.get("names").and_then(Json::as_array) {
        for (slot, v) in [&mut words.base, &mut words.other].into_iter().zip(n) {
            *slot = v.as_str().ok_or("names: two strings")?.to_string();
        }
    }
    for (k, v) in labels(opts)? {
        words.set(&k, &v);
    }
    Ok(words)
}

/// `labels` without the comparison's own words: what the component's
/// options read.
fn without_compare_words(raw: &str) -> Result<String, String> {
    let mut opts = parse(raw, "options")?;
    if let Some(map) = opts.get_mut("labels").and_then(Json::as_object_mut) {
        let words = compare::Labels::default();
        map.retain(|k, _| !words.clone().set(k, ""));
    }
    Ok(opts.to_string())
}

impl Viewer {
    /// Read the documents (a JSON array of coverage documents).
    pub fn from_json(documents_json: &str) -> Result<Viewer, String> {
        let documents = documents(documents_json)?;
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
        let key = model_key(query);
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

/// Two sets of coverage documents measured on the same ODD, compared: a
/// baseline and another set.
#[wasm_bindgen]
pub struct Comparison {
    cmp: compare::Comparison,
    names: [String; 2],
    /// Each side's flaky models, by the selection of runs.
    models: RefCell<HashMap<String, [CasePredictor; 2]>>,
}

impl Comparison {
    /// Read both sets (each a JSON array of coverage documents); `names`
    /// are what the views call them.
    pub fn from_json(base: &str, other: &str, names: [&str; 2]) -> Result<Comparison, String> {
        let (a, b) = (documents(base)?, documents(other)?);
        Ok(Comparison {
            cmp: compare::Comparison::new(&a, &b),
            names: names.map(String::from),
            models: RefCell::new(HashMap::new()),
        })
    }

    fn with_models<T>(&self, query: &CaseQuery, f: impl FnOnce(&[CasePredictor; 2]) -> T) -> T {
        let key = model_key(query);
        let mut models = self.models.borrow_mut();
        let pair = models.entry(key).or_insert_with(|| {
            [Side::Base, Side::Other]
                .map(|s| CasePredictor::new(&self.cmp.cases_of(s), query).with_detail(false))
        });
        f(pair)
    }

    pub fn coverage_html_str(&self, query: &str, options: &str) -> Result<String, String> {
        let raw = parse(options, "options")?;
        let q = Query::from_json(&parse(query, "query")?)?;
        let m = self.cmp.matrix(&q);
        Ok(html::compare::fragment(
            &self.cmp,
            &m,
            &coverage_options(&without_compare_words(options)?)?,
            &compare_words(&raw, &self.names)?,
        ))
    }

    pub fn coverage_json_str(&self, query: &str) -> Result<String, String> {
        let q = Query::from_json(&parse(query, "query")?)?;
        let mut out = self.cmp.matrix(&q).to_json();
        out["names"] = json!(self.names);
        out["odd"] = self.cmp.odd.to_json();
        Ok(out.to_string())
    }

    pub fn cases_html_str(&self, query: &str, options: &str) -> Result<String, String> {
        let raw = parse(options, "options")?;
        let opts = case_options(&without_compare_words(options)?)?;
        let words = compare_words(&raw, &self.names)?;
        let mut background = match raw.get("background").and_then(Json::as_str) {
            Some(b) => Background::parse(b).ok_or("background: delta, base, other or none")?,
            None => Background::Delta,
        };
        if !flag(&raw, "prediction", true) {
            background = Background::None;
        }
        let q = CaseQuery::from_json(&parse(query, "query")?)?;
        let plot = self.cmp.plot(&q);
        if background == Background::None {
            return Ok(case_plot::compare::fragment(
                &self.cmp,
                &plot,
                &opts,
                &words,
                background,
                [None, None],
            ));
        }
        Ok(self.with_models(&plot.plot.query, |[a, b]| {
            case_plot::compare::fragment(
                &self.cmp,
                &plot,
                &opts,
                &words,
                background,
                [Some(a as &dyn Predictor), Some(b as &dyn Predictor)],
            )
        }))
    }

    pub fn cases_json_str(&self, query: &str, prediction: bool) -> Result<String, String> {
        let q = CaseQuery::from_json(&parse(query, "query")?)?;
        let plot = self.cmp.plot(&q);
        let mut out = plot.to_json();
        out["names"] = json!(self.names);
        out["odd"] = self.cmp.odd.to_json();
        if prediction {
            self.with_models(&plot.plot.query, |models| {
                let grids: Vec<Result<_, String>> =
                    models.iter().map(|m| m.grid(&plot.plot)).collect();
                let as_json = |g: &Result<openodd_coverage_view::prediction::Grid, String>| match g
                {
                    Ok(g) => g.to_json(),
                    Err(why) => json!({"unavailable": why}),
                };
                out["prediction"] = json!({
                    "base": as_json(&grids[0]),
                    "other": as_json(&grids[1]),
                    "delta": match (&grids[0], &grids[1]) {
                        (Ok(a), Ok(b)) => as_json(&case_plot::compare::delta_grid(a, b)),
                        (Err(e), _) | (_, Err(e)) => json!({"unavailable": e}),
                    },
                });
                let refs: Vec<&Case> = plot.plot.cases.iter().collect();
                let (pa, pb) = (
                    models[0].pass_probabilities(&refs),
                    models[1].pass_probabilities(&refs),
                );
                if let Some(cases) = out["cases"].as_array_mut() {
                    for ((j, a), b) in cases.iter_mut().zip(pa).zip(pb) {
                        j["predicted_pass_probability"] = json!({"base": a, "other": b});
                    }
                }
            });
        }
        Ok(out.to_string())
    }
}

#[wasm_bindgen]
impl Comparison {
    /// Read two sets of coverage documents (each a JSON array): the
    /// baseline, the other set, and what to call each (`""`: "Baseline" /
    /// "Compared").
    #[wasm_bindgen(constructor)]
    pub fn new(
        base_json: &str,
        other_json: &str,
        base_name: &str,
        other_name: &str,
    ) -> Result<Comparison, JsError> {
        let words = compare::Labels::default();
        let name = |n: &str, d: &str| {
            if n.is_empty() {
                d.to_string()
            } else {
                n.to_string()
            }
        };
        let (a, b) = (name(base_name, &words.base), name(other_name, &words.other));
        Comparison::from_json(base_json, other_json, [&a, &b]).map_err(js)
    }

    /// Documents read: both sets.
    #[wasm_bindgen(getter)]
    pub fn documents(&self) -> usize {
        self.cmp.cases.documents
    }

    /// Documents of the baseline (the first `split` documents).
    #[wasm_bindgen(getter)]
    pub fn split(&self) -> usize {
        self.cmp.split
    }

    /// Distinct cases with a run on either side.
    #[wasm_bindgen(getter)]
    pub fn cases(&self) -> usize {
        self.cmp.cases.cases.len()
    }

    /// The ODD attributes of the coverage matrix (both sets), as a JSON array.
    pub fn attributes(&self) -> String {
        json!(self.cmp.coverage.attributes()).to_string()
    }

    /// The attributes of the cases' space (both sets), as a JSON array.
    pub fn case_attributes(&self) -> String {
        json!(self.cmp.cases.attributes()).to_string()
    }

    /// Whether the two sets were measured on the same ODD, and where not:
    /// JSON (`same`, each side's ODDs, the attributes that differ).
    pub fn odd(&self) -> String {
        self.cmp.odd.to_json().to_string()
    }

    /// The compared matrix as an HTML fragment (host-driven, as the
    /// [`Viewer`]'s). `options` also takes `names` and the comparison's
    /// `labels`.
    pub fn coverage_html(&self, query_json: &str, options_json: &str) -> Result<String, JsError> {
        self.coverage_html_str(query_json, options_json).map_err(js)
    }

    /// The compared matrix as JSON: per cell its change, both sides' cells.
    pub fn coverage_json(&self, query_json: &str) -> Result<String, JsError> {
        self.coverage_json_str(query_json).map_err(js)
    }

    /// The compared case plot as an HTML fragment. `options.background`:
    /// `"delta"` (the change of the predicted pass rate, the default),
    /// `"base"`, `"other"` or `"none"`.
    pub fn cases_html(&self, query_json: &str, options_json: &str) -> Result<String, JsError> {
        self.cases_html_str(query_json, options_json).map_err(js)
    }

    /// The compared case plot as JSON: per case both sides' runs and the
    /// change; with `prediction`, both sides' grids and their change.
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
