//! The drawn cases of a sweep: every run placed at the values it was drawn
//! with, and what it did.
//!
//! A run that a sampler drew records its *case* in its coverage document
//! (`case`, written by openodd's `CoverageCollector.record_case`): the value
//! drawn for each attribute (`values`, a number or a label), the ODD bucket
//! each falls in (`buckets`), the settings that made it (`overrides`), why it
//! was drawn (`chosen_for`: `coverage`, `instability`, ...), the flakiness
//! predicted for it (`flaky`, or null) and the space it was drawn in
//! (`space`: each attribute's range or labels). With its `outcome`, that is
//! everything a scatter of the sweep needs.
//!
//! Runs with the same overrides (in the same scenario) ran the same case:
//! they are one [`Case`] with several [`CaseRun`]s, which passes, fails, or
//! is flaky when its runs did both ([`Verdict`]). A document without a case,
//! or without an outcome, is not placed; [`CaseSet`] counts them.

use indexmap::IndexMap;
use serde_json::{json, Value as Json};

/// What the runs of a case did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Every run passed.
    Pass,
    /// Every run failed.
    Fail,
    /// Some runs passed and some failed.
    Flaky,
}

impl Verdict {
    pub fn as_str(&self) -> &'static str {
        match self {
            Verdict::Pass => "pass",
            Verdict::Fail => "fail",
            Verdict::Flaky => "flaky",
        }
    }

    /// Where its cases sort: what most needs a look first.
    pub fn rank(&self) -> u8 {
        match self {
            Verdict::Flaky => 0,
            Verdict::Fail => 1,
            Verdict::Pass => 2,
        }
    }
}

/// One run of a case.
#[derive(Debug, Clone, PartialEq)]
pub struct CaseRun {
    /// The document's index in the list it was read from.
    pub document: usize,
    pub passed: bool,
    /// When a failed run failed (`outcome.failed_at`), if recorded.
    pub failed_at: Option<f64>,
    pub environment: Option<String>,
    /// Why this run's case was drawn (`coverage`, `instability`, ...).
    pub chosen_for: String,
    /// The flakiness predicted for it when it was drawn, if any.
    pub flaky: Option<Json>,
    /// What the probes measured at the start of the run (`odd.samples`):
    /// the rows within [`MAX_WINDOW`] seconds, `(t, attribute -> value)`,
    /// only the case's attributes and only values that are not null.
    pub start: Vec<(f64, IndexMap<String, Json>)>,
}

/// How much of a run's start is kept to measure it by: the longest
/// measurement window a query may ask for, in seconds.
pub const MAX_WINDOW: f64 = 10.0;

/// The measurement window when a query names none: an attribute's measured
/// value is its first sample within this many seconds of the start.
pub const DEFAULT_WINDOW: f64 = 1.0;

impl CaseRun {
    /// What the run measured: each attribute's first sample at or before
    /// `window` seconds (from t = 0) that has a value.
    pub fn measured(&self, window: f64) -> IndexMap<String, Json> {
        let mut out: IndexMap<String, Json> = IndexMap::new();
        for (t, row) in &self.start {
            if *t > window + 1e-9 {
                break;
            }
            for (k, v) in row {
                if !out.contains_key(k) {
                    out.insert(k.clone(), v.clone());
                }
            }
        }
        out
    }
}

/// Which values the cases are placed by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Basis {
    /// What the sampler drew (`case.values`): what the run aimed at.
    #[default]
    Drawn,
    /// What the probes measured at the start of the run (`odd.samples`).
    Measured,
}

impl Basis {
    pub fn as_str(&self) -> &'static str {
        match self {
            Basis::Drawn => "drawn",
            Basis::Measured => "measured",
        }
    }

    pub fn parse(s: &str) -> Option<Basis> {
        match s {
            "drawn" => Some(Basis::Drawn),
            "measured" => Some(Basis::Measured),
            _ => None,
        }
    }

    pub fn other(&self) -> Basis {
        match self {
            Basis::Drawn => Basis::Measured,
            Basis::Measured => Basis::Drawn,
        }
    }
}

/// The rows of a document's `odd.samples` within [`MAX_WINDOW`], keeping
/// `wanted` attributes with a value.
fn start_samples(doc: &Json, wanted: &[&String]) -> Vec<(f64, IndexMap<String, Json>)> {
    let Some(table) = doc.pointer("/odd/samples").filter(|t| t.is_object()) else {
        return Vec::new();
    };
    let names: Vec<Option<String>> = table
        .get("attributes")
        .and_then(Json::as_array)
        .map(|a| {
            a.iter()
                .map(|x| {
                    x.get("name")
                        .and_then(Json::as_str)
                        .or(x.as_str())
                        .map(String::from)
                })
                .collect()
        })
        .unwrap_or_default();
    // Columns: t, lat, lon, then one per attribute.
    let columns: Vec<(usize, String)> = names
        .iter()
        .enumerate()
        .filter_map(|(i, n)| {
            n.clone()
                .filter(|n| wanted.contains(&n))
                .map(|n| (i + 3, n))
        })
        .collect();
    let mut out = Vec::new();
    for row in table
        .get("rows")
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
    {
        let Some(t) = row.get(0).and_then(Json::as_f64) else {
            continue;
        };
        if t > MAX_WINDOW {
            break;
        }
        let values: IndexMap<String, Json> = columns
            .iter()
            .filter_map(|(i, n)| {
                let v = row.get(*i)?;
                (!v.is_null()).then(|| (n.clone(), v.clone()))
            })
            .collect();
        if !values.is_empty() {
            out.push((t, values));
        }
    }
    out
}

/// A distinct case: its drawn values, and every run of it.
#[derive(Debug, Clone, PartialEq)]
pub struct Case {
    /// What identifies it: its scenario and overrides.
    pub key: String,
    pub scenario: String,
    /// Its index in the batch it was drawn in (its first run's).
    pub index: Option<u64>,
    /// Attribute -> the value drawn: a JSON number, or a label.
    pub values: IndexMap<String, Json>,
    /// Attribute -> the ODD bucket of that value.
    pub buckets: IndexMap<String, String>,
    pub overrides: Vec<String>,
    /// The space it was drawn in (`case.space`), as recorded: what a model
    /// of its outcomes codes it in.
    pub space: Option<Json>,
    /// Which values `values` (and `buckets`) are: drawn, unless a query
    /// asked for the measured ones ([`CaseSet::selected`]).
    pub basis: Basis,
    /// The values of the other basis: the measured ones (the mean over its
    /// runs, or the most common label) when `values` are drawn, and the
    /// drawn ones when they are measured. Empty when the runs recorded no
    /// samples.
    pub other: IndexMap<String, Json>,
    /// In the order of the documents.
    pub runs: Vec<CaseRun>,
}

impl Case {
    pub fn passes(&self) -> usize {
        self.runs.iter().filter(|r| r.passed).count()
    }

    pub fn fails(&self) -> usize {
        self.runs.len() - self.passes()
    }

    pub fn verdict(&self) -> Verdict {
        match (self.passes(), self.fails()) {
            (_, 0) => Verdict::Pass,
            (0, _) => Verdict::Fail,
            _ => Verdict::Flaky,
        }
    }

    /// Why its runs were drawn, each reason once, in order.
    pub fn chosen_for(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for r in &self.runs {
            if !out.contains(&r.chosen_for) {
                out.push(r.chosen_for.clone());
            }
        }
        out
    }

    /// Whether a flaky search drew it (a run of it was chosen for instability).
    pub fn for_instability(&self) -> bool {
        self.runs.iter().any(|r| r.chosen_for == "instability")
    }

    /// The latest flakiness predicted for it, if any run carries one.
    pub fn flaky(&self) -> Option<&Json> {
        self.runs.iter().rev().find_map(|r| r.flaky.as_ref())
    }

    /// Its value of `attribute` as a number, on a numeric axis.
    pub fn number(&self, attribute: &str) -> Option<f64> {
        match self.values.get(attribute)? {
            Json::Number(n) => n.as_f64().filter(|v| v.is_finite()),
            _ => None,
        }
    }

    /// Its value of `attribute` as a label, on a categorical axis.
    pub fn label(&self, attribute: &str) -> Option<String> {
        self.values.get(attribute).map(value_text)
    }

    pub fn to_json(&self) -> Json {
        json!({
            "key": self.key,
            "scenario": self.scenario,
            "index": self.index,
            "values": self.values,
            "buckets": self.buckets,
            "overrides": self.overrides,
            "runs": self.runs.iter().map(|r| json!({
                "document": r.document,
                "passed": r.passed,
                "failed_at": r.failed_at,
                "environment": r.environment,
                "chosen_for": r.chosen_for,
                "flaky": r.flaky,
            })).collect::<Vec<_>>(),
            "passes": self.passes(),
            "fails": self.fails(),
            "verdict": self.verdict().as_str(),
            "chosen_for": self.chosen_for(),
            "instability": self.for_instability(),
            "flaky": self.flaky(),
            "basis": self.basis.as_str(),
            "other_values": self.other,
        })
    }
}

/// A value as text: a label as it is, anything else as JSON.
pub fn value_text(v: &Json) -> String {
    match v {
        Json::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// How an attribute is laid out along an axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AxisKind {
    /// A linear scale.
    Numeric,
    /// Evenly spaced bands, one per label.
    Categorical,
}

impl AxisKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            AxisKind::Numeric => "numeric",
            AxisKind::Categorical => "categorical",
        }
    }
}

/// An ODD bucket of a numeric attribute: `[low, high)` (or `]`).
#[derive(Debug, Clone, PartialEq)]
pub struct Span {
    pub label: String,
    pub low: f64,
    pub high: f64,
}

/// An attribute of the cases' space, as an axis.
#[derive(Debug, Clone, PartialEq)]
pub struct CaseAxis {
    pub name: String,
    pub unit: String,
    pub text: String,
    pub kind: AxisKind,
    /// The space's range (numeric), over every document that gives one.
    pub range: Option<(f64, f64)>,
    /// The drawn values' extent (numeric).
    pub extent: Option<(f64, f64)>,
    /// The bands (categorical): the space's labels in order, then any other
    /// value drawn, in the order first seen.
    pub labels: Vec<String>,
    /// The ODD's buckets of it (numeric), from its cover item, in order:
    /// where the grid lines go.
    pub spans: Vec<Span>,
}

impl CaseAxis {
    /// The values a numeric axis spans: the space's range, widened to the
    /// values drawn; or those values' extent; never empty.
    pub fn domain(&self) -> (f64, f64) {
        let (mut lo, mut hi) = match (self.range, self.extent) {
            (Some(r), Some(e)) => (r.0.min(e.0), r.1.max(e.1)),
            (Some(r), None) => r,
            (None, Some(e)) => {
                let pad = (e.1 - e.0) * 0.05;
                (e.0 - pad, e.1 + pad)
            }
            (None, None) => (0.0, 1.0),
        };
        if hi - lo <= f64::EPSILON * lo.abs().max(1.0) {
            let pad = lo.abs().max(1.0) * 0.1;
            lo -= pad;
            hi += pad;
        }
        (lo, hi)
    }

    /// The bucket edges inside the domain, sorted, each once.
    pub fn edges(&self) -> Vec<f64> {
        let (lo, hi) = self.domain();
        let mut out: Vec<f64> = self
            .spans
            .iter()
            .flat_map(|s| [s.low, s.high])
            .filter(|e| *e >= lo && *e <= hi)
            .collect();
        out.sort_by(f64::total_cmp);
        out.dedup();
        out
    }

    pub fn to_json(&self) -> Json {
        let numeric = self.kind == AxisKind::Numeric;
        json!({
            "name": self.name,
            "unit": self.unit,
            "text": self.text,
            "kind": self.kind.as_str(),
            "range": self.range.map(|r| [r.0, r.1]),
            "domain": numeric.then(|| { let d = self.domain(); [d.0, d.1] }),
            "labels": self.labels,
            "buckets": self.spans.iter()
                .map(|s| json!({"label": s.label, "low": s.low, "high": s.high}))
                .collect::<Vec<_>>(),
        })
    }
}

/// The label of the span a number falls in: `[low, high)`, the last one
/// closed; `<below>` / `<above>` outside them all.
pub fn span_label(spans: &[Span], v: f64) -> String {
    let n = spans.len();
    for (i, s) in spans.iter().enumerate() {
        if v >= s.low && (v < s.high || (i + 1 == n && v <= s.high)) {
            return s.label.clone();
        }
    }
    let lowest = spans.iter().map(|s| s.low).fold(f64::INFINITY, f64::min);
    if v < lowest {
        crate::data::BELOW_RANGE.into()
    } else {
        crate::data::ABOVE_RANGE.into()
    }
}

/// Parse an interval bucket label, `[low, high)` or `[low, high]`.
pub fn parse_span(label: &str) -> Option<(f64, f64)> {
    let t = label.trim();
    let inner = t.strip_prefix(['[', '('])?.strip_suffix([']', ')'])?;
    let (a, b) = inner.split_once(',')?;
    let (a, b) = (a.trim().parse::<f64>().ok()?, b.trim().parse::<f64>().ok()?);
    (a.is_finite() && b.is_finite() && a < b).then_some((a, b))
}

/// Every case of a set of coverage documents.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CaseSet {
    /// In the order of their first runs.
    pub cases: Vec<Case>,
    /// Attribute -> its axis: the space's attributes in the order first seen,
    /// then any other attribute a case has a value of.
    pub axes: IndexMap<String, CaseAxis>,
    /// Documents read.
    pub documents: usize,
    /// Documents without a case (a run no sampler drew), and documents with
    /// a case but no outcome: neither is placed.
    pub without_case: usize,
    pub without_outcome: usize,
}

fn strings(v: Option<&Json>) -> Vec<String> {
    v.and_then(Json::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

fn environment(raw: Option<&Json>) -> Option<String> {
    match raw {
        None | Some(Json::Null) => None,
        Some(Json::String(e)) => Some(e.clone()),
        Some(other) => Some(other.to_string()),
    }
}

impl CaseSet {
    /// Read the documents (the contents of coverage files).
    pub fn from_documents(documents: &[Json]) -> Self {
        let mut set = CaseSet {
            documents: documents.len(),
            ..Default::default()
        };
        // Per axis: the space said numeric (Some(true)) or categorical.
        let mut declared: IndexMap<String, bool> = IndexMap::new();
        let mut index: IndexMap<String, usize> = IndexMap::new();
        for (d, doc) in documents.iter().enumerate() {
            let Some(case) = doc.get("case").filter(|c| c.is_object()) else {
                set.without_case += 1;
                continue;
            };
            let outcome = doc.get("outcome").filter(|o| o.is_object());
            let Some(passed) = outcome
                .and_then(|o| o.get("passed"))
                .and_then(Json::as_bool)
            else {
                set.without_outcome += 1;
                continue;
            };
            let values: IndexMap<String, Json> = case
                .get("values")
                .and_then(Json::as_object)
                .map(|o| o.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                .unwrap_or_default();
            let buckets: IndexMap<String, String> = case
                .get("buckets")
                .and_then(Json::as_object)
                .map(|o| o.iter().map(|(k, v)| (k.clone(), value_text(v))).collect())
                .unwrap_or_default();
            let overrides = strings(case.get("overrides"));
            let scenario = doc
                .get("scenario")
                .and_then(Json::as_str)
                .unwrap_or("")
                .to_string();
            // The same settings are the same case; without settings, the
            // same values are.
            let what = if overrides.is_empty() {
                Json::Object(values.clone().into_iter().collect()).to_string()
            } else {
                overrides.join("\n")
            };
            let key = format!("{scenario}\u{1f}{what}");
            let run = CaseRun {
                document: d,
                passed,
                failed_at: outcome
                    .and_then(|o| o.get("failed_at"))
                    .and_then(Json::as_f64),
                environment: environment(outcome.and_then(|o| o.get("environment"))),
                chosen_for: case
                    .get("chosen_for")
                    .and_then(Json::as_str)
                    .unwrap_or("")
                    .to_string(),
                flaky: case.get("flaky").filter(|f| f.is_object()).cloned(),
                start: start_samples(doc, &values.keys().collect::<Vec<_>>()),
            };
            // The space: each attribute's range or labels.
            for dim in case
                .get("space")
                .and_then(Json::as_array)
                .into_iter()
                .flatten()
            {
                let Some(name) = dim.get("name").and_then(Json::as_str) else {
                    continue;
                };
                let axis = set.axis(name);
                if let Some(r) = dim.get("range").and_then(Json::as_array) {
                    if let (Some(lo), Some(hi)) = (
                        r.first().and_then(Json::as_f64),
                        r.get(1).and_then(Json::as_f64),
                    ) {
                        if lo.is_finite() && hi.is_finite() && lo <= hi {
                            axis.range = Some(match axis.range {
                                Some((a, b)) => (a.min(lo), b.max(hi)),
                                None => (lo, hi),
                            });
                        }
                    }
                    declared.entry(name.to_string()).or_insert(true);
                } else if dim.get("labels").is_some() {
                    for l in strings(dim.get("labels")) {
                        if !axis.labels.contains(&l) {
                            axis.labels.push(l);
                        }
                    }
                    declared.entry(name.to_string()).or_insert(false);
                }
            }
            for name in values.keys() {
                set.axis(name);
            }
            set.read_items(doc);
            match index.get(&key) {
                Some(&i) => set.cases[i].runs.push(run),
                None => {
                    index.insert(key.clone(), set.cases.len());
                    set.cases.push(Case {
                        key,
                        scenario,
                        index: case.get("index").and_then(Json::as_u64),
                        values,
                        buckets,
                        overrides,
                        space: case.get("space").filter(|s| s.is_array()).cloned(),
                        basis: Basis::Drawn,
                        other: IndexMap::new(),
                        runs: vec![run],
                    });
                }
            }
        }
        // Numeric when the space says so, or (without a space) when every
        // value drawn is a number.
        let names: Vec<String> = set.axes.keys().cloned().collect();
        for name in names {
            let numeric = declared.get(&name).copied().unwrap_or_else(|| {
                let mut any = false;
                let all = set.cases.iter().all(|c| match c.values.get(&name) {
                    None => true,
                    Some(Json::Number(_)) => {
                        any = true;
                        true
                    }
                    Some(_) => false,
                });
                all && any
            });
            let mut extent: Option<(f64, f64)> = None;
            let mut seen: Vec<String> = Vec::new();
            for c in &set.cases {
                if numeric {
                    if let Some(v) = c.number(&name) {
                        extent = Some(match extent {
                            Some((a, b)) => (a.min(v), b.max(v)),
                            None => (v, v),
                        });
                    }
                } else if let Some(l) = c.label(&name) {
                    if !seen.contains(&l) {
                        seen.push(l);
                    }
                }
            }
            let axis = &mut set.axes[&name];
            if numeric {
                axis.kind = AxisKind::Numeric;
                axis.extent = extent;
                axis.labels.clear();
            } else {
                axis.kind = AxisKind::Categorical;
                for l in seen {
                    if !axis.labels.contains(&l) {
                        axis.labels.push(l);
                    }
                }
                axis.spans.clear();
            }
        }
        set
    }

    fn axis(&mut self, name: &str) -> &mut CaseAxis {
        self.axes
            .entry(name.to_string())
            .or_insert_with(|| CaseAxis {
                name: name.to_string(),
                unit: String::new(),
                text: String::new(),
                kind: AxisKind::Categorical,
                range: None,
                extent: None,
                labels: Vec::new(),
                spans: Vec::new(),
            })
    }

    /// The unit, text and bucket edges of each attribute's cover item.
    fn read_items(&mut self, doc: &Json) {
        for item in doc
            .get("items")
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
        {
            let Some(name) = item.get("attribute").and_then(Json::as_str) else {
                continue;
            };
            let Some(axis) = self.axes.get_mut(name) else {
                continue;
            };
            let text = |k: &str| item.get(k).and_then(Json::as_str).unwrap_or("").to_string();
            if axis.unit.is_empty() {
                axis.unit = text("unit");
            }
            if axis.text.is_empty() {
                axis.text = text("text");
            }
            for label in strings(item.get("buckets")) {
                if let Some((low, high)) = parse_span(&label) {
                    if !axis.spans.iter().any(|s| s.label == label) {
                        axis.spans.push(Span { label, low, high });
                    }
                }
            }
        }
    }

    /// The attributes, in the order first seen.
    pub fn attributes(&self) -> Vec<String> {
        self.axes.keys().cloned().collect()
    }

    /// The scenarios of the cases, sorted.
    pub fn scenarios(&self) -> Vec<String> {
        let mut out: Vec<String> = self.cases.iter().map(|c| c.scenario.clone()).collect();
        out.sort();
        out.dedup();
        out
    }

    /// Runs placed (with a case and an outcome).
    pub fn runs(&self) -> usize {
        self.cases.iter().map(|c| c.runs.len()).sum()
    }

    /// The attributes the cases of `q` (its scenario and environment) have
    /// a value of, in order.
    pub fn free_attributes(&self, q: &CaseQuery) -> Vec<String> {
        let cases = self.selected(q);
        self.axes
            .keys()
            .filter(|a| cases.iter().any(|c| c.values.contains_key(*a)))
            .cloned()
            .collect()
    }

    /// The cases of `q`: only their runs of its scenario and environment,
    /// and only cases left with a run.
    pub fn selected(&self, q: &CaseQuery) -> Vec<Case> {
        self.cases
            .iter()
            .filter(|c| q.scenario.as_ref().is_none_or(|s| &c.scenario == s))
            .filter_map(|c| {
                let runs: Vec<CaseRun> = c
                    .runs
                    .iter()
                    .filter(|r| q.environment.as_ref().is_none_or(|e| &r.environment == e))
                    .cloned()
                    .collect();
                if runs.is_empty() {
                    return None;
                }
                let mut case = Case { runs, ..c.clone() };
                let measured = self.measured_values(&case, q.window());
                match q.basis {
                    Basis::Drawn => case.other = measured,
                    // A case nothing was measured of has nothing to place.
                    Basis::Measured if measured.is_empty() => return None,
                    Basis::Measured => {
                        case.buckets = measured
                            .iter()
                            .map(|(k, v)| (k.clone(), self.bucket_of(k, v)))
                            .collect();
                        case.other = std::mem::replace(&mut case.values, measured);
                        case.basis = Basis::Measured;
                    }
                }
                Some(case)
            })
            .collect()
    }

    /// A case's measured values: per attribute, the mean of its runs'
    /// measured numbers, or their most common label (the first seen on a
    /// tie); only the attributes it was drawn with.
    pub fn measured_values(&self, case: &Case, window: f64) -> IndexMap<String, Json> {
        let per_run: Vec<IndexMap<String, Json>> =
            case.runs.iter().map(|r| r.measured(window)).collect();
        let mut out = IndexMap::new();
        for name in case.values.keys() {
            let vals: Vec<&Json> = per_run.iter().filter_map(|m| m.get(name)).collect();
            if vals.is_empty() {
                continue;
            }
            let numeric = self
                .axes
                .get(name)
                .is_some_and(|a| a.kind == AxisKind::Numeric);
            let nums: Vec<f64> = vals.iter().filter_map(|v| v.as_f64()).collect();
            if numeric && !nums.is_empty() {
                let mean = nums.iter().sum::<f64>() / nums.len() as f64;
                out.insert(name.clone(), serde_json::json!(mean));
            } else {
                let mut counts: IndexMap<String, usize> = IndexMap::new();
                for v in &vals {
                    *counts.entry(value_text(v)).or_default() += 1;
                }
                let best = counts
                    .iter()
                    .max_by_key(|(_, n)| **n)
                    .map(|(l, _)| l.clone())
                    .unwrap_or_default();
                // Keep the label's form: the first value with that text.
                let v = vals
                    .iter()
                    .find(|v| value_text(v) == best)
                    .map(|v| (*v).clone())
                    .unwrap_or(Json::String(best));
                out.insert(name.clone(), v);
            }
        }
        out
    }

    /// The ODD bucket a value falls in: a numeric attribute's span (below or
    /// above them all: `<below>` / `<above>`), or the label itself.
    pub fn bucket_of(&self, attribute: &str, value: &Json) -> String {
        let Some(axis) = self.axes.get(attribute) else {
            return value_text(value);
        };
        match (axis.kind, value.as_f64()) {
            (AxisKind::Numeric, Some(v)) if !axis.spans.is_empty() => span_label(&axis.spans, v),
            _ => value_text(value),
        }
    }

    /// The plot `query` asks for. An axis on an attribute no selected case
    /// has is not used: the first free numeric attributes are, then any.
    pub fn plot(&self, query: &CaseQuery) -> Plot {
        let mut q = query.clone();
        let free = self.free_attributes(&q);
        // Numeric attributes make the better default axes.
        let mut preferred: Vec<&String> = free
            .iter()
            .filter(|a| self.axes[*a].kind == AxisKind::Numeric)
            .collect();
        preferred.extend(
            free.iter()
                .filter(|a| self.axes[*a].kind != AxisKind::Numeric),
        );
        let pick = |asked: &Option<String>, taken: Option<&String>| -> Option<String> {
            match asked {
                Some(a) if free.contains(a) && Some(a) != taken => Some(a.clone()),
                _ => preferred
                    .iter()
                    .find(|a| Some(**a) != taken)
                    .map(|a| (*a).clone()),
            }
        };
        let x_name = pick(&q.x, None);
        let y_name = x_name.as_ref().and_then(|x| pick(&q.y, Some(x)));
        q.x = x_name.clone();
        q.y = y_name.clone();
        let cases = self.selected(&q);
        let mut environments: Vec<Option<String>> = Vec::new();
        for r in cases.iter().flat_map(|c| &c.runs) {
            if !environments.contains(&r.environment) {
                environments.push(r.environment.clone());
            }
        }
        let has = |c: &Case, a: &Option<String>| match a {
            None => true,
            Some(a) => match self.axes[a].kind {
                AxisKind::Numeric => c.number(a).is_some(),
                AxisKind::Categorical => c.values.contains_key(a),
            },
        };
        let (placed, unplaced): (Vec<Case>, Vec<Case>) = cases
            .into_iter()
            .partition(|c| has(c, &x_name) && has(c, &y_name));
        // The axes span the values on this basis (the measured ones may lie
        // outside what was drawn, or be labels the space has not).
        let widen = |name: String| {
            let mut axis = self.axes[&name].clone();
            for c in &placed {
                match axis.kind {
                    AxisKind::Numeric => {
                        // The other basis's values too: the axes stay the
                        // same when the toggle switches, and a line to the
                        // other value stays inside.
                        let other = c.other.get(&name).and_then(Json::as_f64);
                        for v in c.number(&name).into_iter().chain(other) {
                            axis.extent = Some(match axis.extent {
                                Some((a, b)) => (a.min(v), b.max(v)),
                                None => (v, v),
                            });
                        }
                    }
                    AxisKind::Categorical => {
                        if let Some(l) = c.label(&name) {
                            if !axis.labels.contains(&l) {
                                axis.labels.push(l);
                            }
                        }
                    }
                }
            }
            axis
        };
        Plot {
            attributes: free,
            x: x_name.map(widen),
            y: y_name.map(widen),
            cases: placed,
            unplaced: unplaced.len(),
            without_case: self.without_case,
            without_outcome: self.without_outcome,
            environments,
            query: q,
        }
    }
}

/// Which cases a plot shows, and along which attributes.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CaseQuery {
    /// The horizontal attribute; `None` picks the first free one (numeric
    /// first).
    pub x: Option<String>,
    /// The vertical attribute; `None` picks the next.
    pub y: Option<String>,
    /// Only the cases of this scenario.
    pub scenario: Option<String>,
    /// Only the runs of this environment (`Some(None)`: those that recorded
    /// none); all when `None`.
    pub environment: Option<Option<String>>,
    /// Which values the cases are placed by (and the model is fitted on).
    pub basis: Basis,
    /// The measurement window, in seconds from t = 0: an attribute's
    /// measured value is its first sample within it ([`DEFAULT_WINDOW`]
    /// when `None`, at most [`MAX_WINDOW`]).
    pub measured_at: Option<f64>,
}

impl CaseQuery {
    /// The measurement window in force.
    pub fn window(&self) -> f64 {
        self.measured_at
            .unwrap_or(DEFAULT_WINDOW)
            .clamp(0.0, MAX_WINDOW)
    }

    /// A query from JSON: `{"x", "y", "scenario", "environment"}`, every key
    /// optional; `"environment": null` selects the runs that recorded none.
    pub fn from_json(raw: &Json) -> Result<Self, String> {
        let Some(obj) = raw.as_object() else {
            return Err("a query is a JSON object".into());
        };
        let text = |k: &str| -> Result<Option<String>, String> {
            match obj.get(k) {
                None | Some(Json::Null) => Ok(None),
                Some(Json::String(s)) => Ok(Some(s.clone())),
                Some(_) => Err(format!("'{k}' must be a string")),
            }
        };
        let mut q = CaseQuery {
            x: text("x")?.filter(|s| !s.is_empty()),
            y: text("y")?.filter(|s| !s.is_empty()),
            scenario: text("scenario")?,
            environment: None,
            basis: Basis::Drawn,
            measured_at: None,
        };
        if obj.contains_key("environment") {
            q.environment = Some(text("environment")?);
        }
        if let Some(b) = text("basis")? {
            q.basis = Basis::parse(&b).ok_or("'basis' is \"drawn\" or \"measured\"")?;
        }
        if let Some(w) = obj.get("measured_at").filter(|w| !w.is_null()) {
            q.measured_at = Some(
                w.as_f64()
                    .filter(|w| w.is_finite() && (0.0..=MAX_WINDOW).contains(w))
                    .ok_or(format!("'measured_at' must be 0 to {MAX_WINDOW} seconds"))?,
            );
        }
        Ok(q)
    }
}

/// The cases along two attributes.
#[derive(Debug, Clone, PartialEq)]
pub struct Plot {
    pub query: CaseQuery,
    /// Every attribute the selected cases have, for a host's axis pickers.
    pub attributes: Vec<String>,
    pub x: Option<CaseAxis>,
    pub y: Option<CaseAxis>,
    /// The cases placed: with a value of both axes.
    pub cases: Vec<Case>,
    /// Cases selected but without a value of an axis.
    pub unplaced: usize,
    /// Documents not placed: without a case, or without an outcome.
    pub without_case: usize,
    pub without_outcome: usize,
    /// The environments of the runs shown: more than one means runs of
    /// different software are pooled.
    pub environments: Vec<Option<String>>,
}

impl Plot {
    /// How many cases each verdict has.
    pub fn counts(&self) -> IndexMap<&'static str, usize> {
        let mut out: IndexMap<&'static str, usize> = [Verdict::Flaky, Verdict::Fail, Verdict::Pass]
            .iter()
            .map(|v| (v.as_str(), 0))
            .collect();
        for c in &self.cases {
            *out.get_mut(c.verdict().as_str()).unwrap() += 1;
        }
        out
    }

    /// Runs of the cases placed.
    pub fn runs(&self) -> usize {
        self.cases.iter().map(|c| c.runs.len()).sum()
    }

    /// The cases run more than once: flaky first, then failing, then the
    /// most runs.
    pub fn repeated(&self) -> Vec<&Case> {
        let mut out: Vec<&Case> = self.cases.iter().filter(|c| c.runs.len() > 1).collect();
        out.sort_by_key(|c| (c.verdict().rank(), std::cmp::Reverse(c.runs.len())));
        out
    }

    pub fn to_json(&self) -> Json {
        let axis = |a: &Option<CaseAxis>| a.as_ref().map(CaseAxis::to_json);
        json!({
            "x": axis(&self.x),
            "y": axis(&self.y),
            "scenario": self.query.scenario,
            "environment": self.query.environment,
            "basis": self.query.basis.as_str(),
            "measured_at": self.query.window(),
            "attributes": self.attributes,
            "cases": self.cases.iter().map(Case::to_json).collect::<Vec<_>>(),
            "counts": self.counts(),
            "runs": self.runs(),
            "repeated": self.cases.iter().filter(|c| c.runs.len() > 1).count(),
            "unplaced_cases": self.unplaced,
            "documents_without_case": self.without_case,
            "documents_without_outcome": self.without_outcome,
            "environments": self.environments,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(overrides: &[&str], values: Json, passed: Option<bool>, chosen: &str) -> Json {
        let mut d = json!({
            "schema": "openodd.coverage/1", "scenario": "s",
            "items": [{"attribute": "gap", "unit": "m", "buckets": ["[0, 10)", "[10, 20)", "[20, 50]"]}],
            "case": {
                "index": 0, "values": values, "buckets": {}, "overrides": overrides,
                "chosen_for": chosen, "flaky": null,
                "space": [{"name": "gap", "range": [5.0, 30.0]},
                          {"name": "rain", "labels": ["none", "heavy"]}],
            },
        });
        if let Some(p) = passed {
            d["outcome"] = json!({"passed": p});
        }
        d
    }

    #[test]
    fn spans_parse() {
        assert_eq!(parse_span("[0, 10)"), Some((0.0, 10.0)));
        assert_eq!(parse_span(" [-15, -5] "), Some((-15.0, -5.0)));
        assert_eq!(parse_span("heavy"), None);
        assert_eq!(parse_span("[3, 3)"), None);
    }

    #[test]
    fn runs_of_one_case_are_grouped() {
        let docs = vec![
            doc(
                &["a=1"],
                json!({"gap": 12.0, "rain": "none"}),
                Some(true),
                "coverage",
            ),
            doc(
                &["a=1"],
                json!({"gap": 12.0, "rain": "none"}),
                Some(false),
                "coverage",
            ),
            doc(
                &["a=2"],
                json!({"gap": 25.0, "rain": "heavy"}),
                Some(false),
                "instability",
            ),
            doc(
                &["a=3"],
                json!({"gap": 7.0, "rain": "heavy"}),
                None,
                "coverage",
            ),
            json!({"scenario": "s", "case": null, "outcome": {"passed": true}}),
        ];
        let set = CaseSet::from_documents(&docs);
        assert_eq!(set.cases.len(), 2);
        assert_eq!((set.without_case, set.without_outcome), (1, 1));
        assert_eq!(set.cases[0].verdict(), Verdict::Flaky);
        assert_eq!(set.cases[1].verdict(), Verdict::Fail);
        assert!(set.cases[1].for_instability());
        let gap = &set.axes["gap"];
        assert_eq!(gap.kind, AxisKind::Numeric);
        assert_eq!(gap.domain(), (5.0, 30.0));
        assert_eq!(gap.edges(), vec![10.0, 20.0]);
        assert_eq!(gap.unit, "m");
        assert_eq!(set.axes["rain"].kind, AxisKind::Categorical);
        assert_eq!(set.axes["rain"].labels, vec!["none", "heavy"]);
        let plot = set.plot(&CaseQuery::default());
        assert_eq!(plot.x.as_ref().unwrap().name, "gap");
        assert_eq!(plot.y.as_ref().unwrap().name, "rain");
        assert_eq!(plot.repeated().len(), 1);
        assert_eq!(plot.counts()["flaky"], 1);
        let j = plot.to_json();
        assert_eq!(j["cases"][0]["verdict"], "flaky");
        assert_eq!(j["documents_without_case"], 1);
    }

    #[test]
    fn without_a_space_numbers_are_numeric() {
        let mut d = doc(
            &[],
            json!({"speed": 3, "mode": "x"}),
            Some(true),
            "coverage",
        );
        d["case"]["space"] = Json::Null;
        let set = CaseSet::from_documents(&[d]);
        assert_eq!(set.axes["speed"].kind, AxisKind::Numeric);
        assert_eq!(set.axes["mode"].kind, AxisKind::Categorical);
        let (lo, hi) = set.axes["speed"].domain();
        assert!(lo < 3.0 && hi > 3.0);
    }

    #[test]
    fn query_from_json() {
        let q = CaseQuery::from_json(&json!({"x": "a", "y": "", "environment": null})).unwrap();
        assert_eq!(q.x.as_deref(), Some("a"));
        assert_eq!(q.y, None);
        assert_eq!(q.environment, Some(None));
        assert!(CaseQuery::from_json(&json!({"x": 1})).is_err());
    }
}
