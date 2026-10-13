//! Merge coverage files and grade them.
//!
//! OpenSCENARIO DSL says what to collect, not how runs are merged or graded,
//! so this follows the convention coverage tools inherit from hardware
//! verification:
//!
//! * a bucket is covered when its hits reach the item's `target` -- or, for an
//!   item with `cover_by`, its seconds, metres or entries do, counting only
//!   stays of at least `min_stay` when it has one;
//! * an item's grade is its covered buckets over its buckets (those outside the
//!   ODD are not targets); a group's grade is the mean of its items'.
//!
//! Two coverage files describe the same item when the name, the bucket labels,
//! the event and the criteria agree. An item defined differently in different
//! runs is reported once per definition (`name#2`, ...), never merged.
//!
//! Besides hits, a merged bucket counts the runs that hit it, and sums its
//! exposure: seconds spent in it, metres driven in it, times it was entered.
//! A file from before exposure was recorded leaves those unknown for the item.
//!
//! A run that recorded the case it ran (its values, and the flakiness a flaky
//! search predicted when it chose it) and its outcome is kept as a
//! [`CaseRun`]; `crate::flaky::assess` refits the flakiness of every such
//! run over all the runs of its scenario.
//!
//! A run that recorded its outcome and its ODD cells also counts in
//! [`CoverageReport::cell_flakiness`]: per cell, how many of the runs that
//! drove through it failed (`crate::cell_flakiness`).

use indexmap::IndexMap;
use serde_json::{json, Map, Value as Json};

use crate::cell_flakiness::CellFlakiness;
use crate::error::{Error, Result};

/// The schema of the coverage files this library writes and reads.
pub const COVERAGE_SCHEMA: &str = "openodd.coverage/1";
/// Read as [`COVERAGE_SCHEMA`]: the same format under its earlier name.
pub const COVERAGE_SCHEMAS: [&str; 2] = [COVERAGE_SCHEMA, "autoware_carla_scenario.coverage/1"];
/// How long, how far and how often a bucket was in, besides its hits.
pub const EXPOSURE_MEASURES: [&str; 3] = ["seconds", "meters", "entries"];

/// An item or a cross merged over runs.
#[derive(Debug, Clone, PartialEq)]
pub struct MergedEntry {
    pub name: String,
    pub group: String,
    /// `numeric`, `categorical` or `cross`.
    pub kind: String,
    pub event: String,
    pub text: String,
    pub target: f64,
    pub buckets: Vec<String>,
    pub unit: String,
    /// A cross's items.
    pub items: Vec<String>,
    pub hits: IndexMap<String, i64>,
    /// Runs that hit each bucket.
    pub runs: IndexMap<String, i64>,
    /// Bucket -> the documents (their indices in the merge) that hit it.
    pub hit_by: IndexMap<String, Vec<usize>>,
    pub out_of_range: IndexMap<String, i64>,
    pub samples: i64,
    pub ignored: i64,
    /// Buckets outside the ODD: reported, but not targets.
    pub outside: Vec<String>,
    /// Exposure per bucket and measure; `None` once a run did not record it.
    pub exposure: IndexMap<String, Option<IndexMap<String, f64>>>,
    pub cover_by: String,
    pub min_stay: Option<f64>,
    /// With a minimum stay: per measure, the sums over the stays that counted.
    pub counted: IndexMap<String, IndexMap<String, f64>>,
}

impl MergedEntry {
    /// What the bucket has of the measure `target` counts.
    pub fn amount(&self, bucket: &str) -> f64 {
        if self.min_stay.is_some() {
            return self
                .counted
                .get(&self.cover_by)
                .and_then(|m| m.get(bucket))
                .copied()
                .unwrap_or(0.0);
        }
        if self.cover_by == "hits" {
            return *self.hits.get(bucket).unwrap_or(&0) as f64;
        }
        match self.exposure.get(&self.cover_by) {
            Some(Some(sums)) => sums.get(bucket).copied().unwrap_or(0.0),
            _ => 0.0,
        }
    }

    pub fn is_covered(&self, bucket: &str) -> bool {
        self.amount(bucket) >= self.target - 1e-9
    }

    /// The buckets coverage aims at: all but those outside the ODD.
    pub fn targets(&self) -> Vec<String> {
        self.buckets
            .iter()
            .filter(|b| !self.outside.contains(b))
            .cloned()
            .collect()
    }

    pub fn covered(&self) -> Vec<String> {
        self.targets()
            .into_iter()
            .filter(|b| self.is_covered(b))
            .collect()
    }

    pub fn holes(&self) -> Vec<String> {
        self.targets()
            .into_iter()
            .filter(|b| !self.is_covered(b))
            .collect()
    }

    /// Covered targets over targets; 1.0 when the ODD leaves no target.
    pub fn grade(&self) -> f64 {
        let targets = self.targets();
        if targets.is_empty() {
            1.0
        } else {
            self.covered().len() as f64 / targets.len() as f64
        }
    }

    /// The bucket's exposure, `None` where unknown.
    pub fn exposure_of(&self, bucket: &str) -> IndexMap<String, Option<f64>> {
        EXPOSURE_MEASURES
            .iter()
            .map(|m| {
                let v = match self.exposure.get(*m) {
                    // Rounded as the reference rounds it, before it is printed.
                    Some(Some(sums)) => Some(round3(sums.get(bucket).copied().unwrap_or(0.0))),
                    _ => None,
                };
                (m.to_string(), v)
            })
            .collect()
    }

    fn add_exposure(&mut self, raw: &Json) {
        if let Some(Json::Object(counted)) = raw.get("counted") {
            for (measure, by_bucket) in counted {
                let mine = self.counted.entry(measure.clone()).or_default();
                for (bucket, amount) in by_bucket.as_object().into_iter().flatten() {
                    *mine.entry(bucket.clone()).or_default() += amount.as_f64().unwrap_or(0.0);
                }
            }
        }
        for measure in EXPOSURE_MEASURES {
            let slot = self
                .exposure
                .entry(measure.to_string())
                .or_insert_with(|| Some(IndexMap::new()));
            let Some(sums) = slot else { continue };
            match raw.get(measure) {
                Some(Json::Object(counts)) => {
                    for (bucket, amount) in counts {
                        *sums.entry(bucket.clone()).or_default() += amount.as_f64().unwrap_or(0.0);
                    }
                }
                _ => *slot = None,
            }
        }
    }

    pub fn to_json(&self) -> Json {
        let buckets: Vec<Json> = self
            .buckets
            .iter()
            .map(|b| {
                let mut o = Map::new();
                o.insert("bucket".into(), json!(b));
                o.insert("hits".into(), json!(self.hits.get(b).copied().unwrap_or(0)));
                o.insert("runs".into(), json!(self.runs.get(b).copied().unwrap_or(0)));
                for (m, v) in self.exposure_of(b) {
                    o.insert(m, v.map(round3).map(Json::from).unwrap_or(Json::Null));
                }
                o.insert("amount".into(), json!(self.amount(b)));
                o.insert("covered".into(), json!(self.is_covered(b)));
                o.insert("outside_odd".into(), json!(self.outside.contains(b)));
                Json::Object(o)
            })
            .collect();
        let mut out = Map::new();
        for (k, v) in [
            ("name", json!(self.name)),
            ("group", json!(self.group)),
            ("kind", json!(self.kind)),
            ("event", json!(self.event)),
            ("text", json!(self.text)),
            ("target", json!(self.target)),
            ("cover_by", json!(self.cover_by)),
            ("min_stay", json!(self.min_stay)),
            ("grade", json!(self.grade())),
            ("buckets", Json::Array(buckets)),
        ] {
            out.insert(k.into(), v);
        }
        if self.kind == "cross" {
            out.insert("items".into(), json!(self.items));
        } else {
            out.insert("unit".into(), json!(self.unit));
            out.insert("samples".into(), json!(self.samples));
            out.insert("ignored".into(), json!(self.ignored));
            out.insert("out_of_range".into(), json!(self.out_of_range));
        }
        Json::Object(out)
    }
}

fn round3(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

/// How long the runs of one ODD spent inside and outside it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct OddExposure {
    pub name: String,
    pub text: String,
    pub runs: i64,
    /// Runs that left the ODD at least once.
    pub runs_outside: i64,
    /// Ticks and seconds inside, inside only because values were missing, and outside.
    pub ticks: IndexMap<String, i64>,
    pub seconds: IndexMap<String, f64>,
    /// Module -> ticks it failed, and ticks its verdict rested on missing values.
    pub module_ticks: IndexMap<String, IndexMap<String, i64>>,
    /// Attributes the ODD monitors but has no buckets for.
    pub unmeasured: Vec<String>,
    /// The git sources the ODD was read from.
    pub sources: Vec<Json>,
    /// The revisions of the ODD the runs measured (more than one: different ODDs).
    pub revisions: Vec<String>,
    /// (scenario, outside seconds, out intervals) of the runs that left it.
    pub excursions: Vec<(String, f64, Json)>,
}

const VERDICTS: [&str; 3] = ["inside", "assumed", "outside"];

impl OddExposure {
    fn new(name: &str) -> Self {
        OddExposure {
            name: name.into(),
            ticks: VERDICTS.iter().map(|k| (k.to_string(), 0)).collect(),
            seconds: VERDICTS.iter().map(|k| (k.to_string(), 0.0)).collect(),
            ..Default::default()
        }
    }

    fn add(&mut self, scenario: &str, raw: &Json) {
        self.runs += 1;
        if self.text.is_empty() {
            self.text = raw.get("text").and_then(Json::as_str).unwrap_or("").into();
        }
        for key in VERDICTS {
            *self.ticks.get_mut(key).unwrap() += raw
                .pointer(&format!("/ticks/{key}"))
                .and_then(Json::as_i64)
                .unwrap_or(0);
            *self.seconds.get_mut(key).unwrap() += raw
                .pointer(&format!("/seconds/{key}"))
                .and_then(Json::as_f64)
                .unwrap_or(0.0);
        }
        for (module, counts) in raw
            .get("module_ticks")
            .and_then(Json::as_object)
            .into_iter()
            .flatten()
        {
            let mine = self.module_ticks.entry(module.clone()).or_insert_with(|| {
                [
                    ("failed_ticks".to_string(), 0),
                    ("missing_ticks".to_string(), 0),
                ]
                .into_iter()
                .collect()
            });
            for (key, v) in mine.iter_mut() {
                *v += counts.get(key).and_then(Json::as_i64).unwrap_or(0);
            }
        }
        for name in raw
            .get("unmeasured")
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(n) = name.as_str() {
                if !self.unmeasured.iter().any(|u| u == n) {
                    self.unmeasured.push(n.into());
                }
            }
        }
        for source in raw
            .get("sources")
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
        {
            if !self.sources.contains(source) {
                self.sources.push(source.clone());
            }
        }
        if let Some(rev) = raw.get("revision").and_then(Json::as_str) {
            if !self.revisions.iter().any(|r| r == rev) {
                self.revisions.push(rev.into());
            }
        }
        let outside = raw
            .pointer("/seconds/outside")
            .and_then(Json::as_f64)
            .unwrap_or(0.0);
        if raw
            .pointer("/ticks/outside")
            .and_then(Json::as_i64)
            .unwrap_or(0)
            > 0
        {
            self.runs_outside += 1;
            self.excursions.push((
                scenario.into(),
                outside,
                raw.get("out_intervals").cloned().unwrap_or(json!([])),
            ));
        }
    }

    pub fn to_json(&self) -> Json {
        json!({
            "name": self.name,
            "text": self.text,
            "runs": self.runs,
            "runs_outside": self.runs_outside,
            "ticks": self.ticks,
            "seconds": self.seconds.iter().map(|(k, v)| (k.clone(), round3(*v))).collect::<IndexMap<_, _>>(),
            "module_ticks": self.module_ticks,
            "unmeasured": self.unmeasured,
            "sources": self.sources,
            "revisions": self.revisions,
            "excursions": self.excursions.iter().map(|(s, t, i)| json!({
                "scenario": s, "outside_seconds": round3(*t), "intervals": i,
            })).collect::<Vec<_>>(),
        })
    }
}

/// Coverage merged over a number of runs.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CoverageReport {
    pub runs: i64,
    pub scenarios: IndexMap<String, i64>,
    pub entries: Vec<MergedEntry>,
    /// ODD name -> how the runs measured against it fared.
    pub odds: IndexMap<String, OddExposure>,
    /// The runs that recorded the case they ran, in the order merged.
    pub cases: Vec<CaseRun>,
    /// Per scenario, environment and ODD attribute list: the runs' outcomes
    /// per ODD cell. Empty when no run recorded both cells and an outcome.
    pub cell_flakiness: Vec<CellFlakiness>,
}

/// How flaky a case is (see `crate::flaky::Metrics`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Flakiness {
    pub pass_probability: f64,
    pub flip_probability: f64,
    pub instability_confidence: f64,
    pub robustness_radius: Option<f64>,
}

impl Flakiness {
    pub fn to_json(&self) -> Json {
        json!({
            "pass_probability": self.pass_probability,
            "flip_probability": self.flip_probability,
            "instability_confidence": self.instability_confidence,
            "robustness_radius": self.robustness_radius,
        })
    }

    fn from_json(raw: &Json) -> Option<Self> {
        let get = |k: &str| raw.get(k).and_then(Json::as_f64);
        Some(Flakiness {
            pass_probability: get("pass_probability")?,
            flip_probability: get("flip_probability")?,
            instability_confidence: get("instability_confidence")?,
            robustness_radius: get("robustness_radius"),
        })
    }
}

/// A run that recorded the case it ran: what the coverage file's `case` and
/// `outcome` hold.
#[derive(Debug, Clone, PartialEq)]
pub struct CaseRun {
    /// The coverage file's index in the merge.
    pub document: usize,
    pub scenario: String,
    /// Parameter -> its value (a JSON number, string or bool).
    pub values: Map<String, Json>,
    /// Parameter -> the bucket drawn for it.
    pub buckets: Map<String, Json>,
    /// `instability` (a flaky search chose it) or `coverage` (the sampler's
    /// own policy did).
    pub chosen_for: String,
    /// The flakiness predicted when the case was chosen for its instability.
    pub predicted: Option<Flakiness>,
    /// The normalized space the case was chosen in, as written.
    pub space: Option<Json>,
    pub passed: Option<bool>,
    /// What all the runs of its scenario and space say of it, once assessed.
    pub assessed: Option<Flakiness>,
}

impl CaseRun {
    fn read(document: usize, scenario: &str, doc: &Json) -> Option<Self> {
        let case = doc.get("case").filter(|c| c.is_object())?;
        let map = |k: &str| {
            case.get(k)
                .and_then(Json::as_object)
                .cloned()
                .unwrap_or_default()
        };
        Some(CaseRun {
            document,
            scenario: scenario.to_string(),
            values: map("values"),
            buckets: map("buckets"),
            chosen_for: case
                .get("chosen_for")
                .and_then(Json::as_str)
                .unwrap_or("coverage")
                .to_string(),
            predicted: case.get("flaky").and_then(Flakiness::from_json),
            space: case.get("space").filter(|s| s.is_array()).cloned(),
            passed: doc
                .get("outcome")
                .and_then(|o| o.get("passed"))
                .and_then(Json::as_bool),
            assessed: None,
        })
    }

    /// Its flakiness, as assessed if it was, else as predicted.
    pub fn flakiness(&self) -> Option<Flakiness> {
        self.assessed.or(self.predicted)
    }

    pub fn to_json(&self) -> Json {
        json!({
            "document": self.document,
            "scenario": self.scenario,
            "values": self.values,
            "buckets": self.buckets,
            "chosen_for": self.chosen_for,
            "predicted": self.predicted.map(|f| f.to_json()),
            "space": self.space,
            "passed": self.passed,
            "assessed": self.assessed.map(|f| f.to_json()),
        })
    }
}

/// The most runs the "Flaky cases" section of the Markdown report lists.
pub const MAX_CASE_ROWS: usize = 20;

/// Above this instability confidence, a case counts as flaky in the report.
pub const FLAKY_CONFIDENCE: f64 = 0.5;

const GROUP_ORDER: [&str; 3] = ["odd", "situation", "scenario"];

impl CoverageReport {
    pub fn groups(&self) -> Vec<String> {
        let mut groups: Vec<String> = Vec::new();
        for e in &self.entries {
            if !groups.contains(&e.group) {
                groups.push(e.group.clone());
            }
        }
        groups.sort_by_key(|g| {
            (
                GROUP_ORDER.iter().position(|x| x == g).unwrap_or(9),
                g.clone(),
            )
        });
        groups
    }

    pub fn group_grade(&self, group: &str) -> f64 {
        let grades: Vec<f64> = self
            .entries
            .iter()
            .filter(|e| e.group == group)
            .map(MergedEntry::grade)
            .collect();
        if grades.is_empty() {
            0.0
        } else {
            grades.iter().sum::<f64>() / grades.len() as f64
        }
    }

    pub fn entry(&self, name: &str) -> Option<&MergedEntry> {
        self.entries.iter().find(|e| e.name == name)
    }

    pub fn to_json(&self) -> Json {
        let mut out = json!({
            "runs": self.runs,
            "scenarios": self.scenarios,
            "odds": self.odds.iter().map(|(k, v)| (k.clone(), v.to_json())).collect::<Map<_, _>>(),
            "cases": self.cases.iter().map(CaseRun::to_json).collect::<Vec<_>>(),
            "groups": self.groups().iter().map(|g| (g.clone(), json!({
                "grade": self.group_grade(g),
                "entries": self.entries.iter().filter(|e| &e.group == g).map(MergedEntry::to_json).collect::<Vec<_>>(),
            }))).collect::<Map<_, _>>(),
        });
        // Only when there is any: a report without outcomes is unchanged.
        if !self.cell_flakiness.is_empty() {
            out["cell_flakiness"] = Json::Array(
                self.cell_flakiness
                    .iter()
                    .map(CellFlakiness::to_json)
                    .collect(),
            );
        }
        out
    }

    pub fn to_markdown(&self, max_holes: usize) -> String {
        let mut lines = vec![
            "# Coverage report".to_string(),
            String::new(),
            format!("Runs merged: {}", self.runs),
            String::new(),
        ];
        let mut scenarios: Vec<(&String, &i64)> = self.scenarios.iter().collect();
        scenarios.sort();
        for (s, n) in scenarios {
            lines.push(format!("- {s}: {n}"));
        }
        for exposure in self.odds.values() {
            lines.extend(odd_markdown(exposure));
        }
        for group in self.groups() {
            let entries: Vec<&MergedEntry> =
                self.entries.iter().filter(|e| e.group == group).collect();
            let title = if group == "odd" {
                "ODD coverage".to_string()
            } else {
                format!("{} coverage", capitalize(&group))
            };
            lines.extend([
                String::new(),
                format!("## {title}: {}", pct1(self.group_grade(&group))),
                String::new(),
                "| Item | Event | Grade | Covered | Holes |".into(),
                "|---|---|---|---|---|".into(),
            ]);
            for e in &entries {
                let holes = e.holes();
                let mut shown = holes
                    .iter()
                    .take(max_holes)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ");
                if holes.len() > max_holes {
                    shown += &format!(", ... ({} more)", holes.len() - max_holes);
                }
                let label = if e.items.is_empty() {
                    e.name.clone()
                } else {
                    format!("{} ({})", e.name, e.items.join(" x "))
                };
                lines.push(format!(
                    "| {label} | {} | {} | {}/{} | {} |",
                    e.event,
                    pct0(e.grade()),
                    e.covered().len(),
                    e.targets().len(),
                    if shown.is_empty() { "-".into() } else { shown }
                ));
            }
            for e in entries.iter().filter(|e| e.kind != "cross") {
                lines.extend([
                    String::new(),
                    format!(
                        "### {}{}",
                        e.name,
                        if e.unit.is_empty() {
                            String::new()
                        } else {
                            format!(" [{}]", e.unit)
                        }
                    ),
                    String::new(),
                ]);
                if !e.text.is_empty() {
                    lines.extend([e.text.clone(), String::new()]);
                }
                if e.cover_by != "hits" || e.min_stay.is_some() {
                    let mut rule = format!(
                        "Covered at {} {} per bucket",
                        fmt_g(e.target),
                        unit_of(&e.cover_by)
                    );
                    if let Some(m) = e.min_stay {
                        rule += &format!(", counting stays of {} s or more", fmt_g(m));
                    }
                    lines.extend([rule + ".", String::new()]);
                }
                let timed = e.event == "tick";
                if timed {
                    lines.extend([
                        "| Bucket | Hits | Runs | Seconds | Meters | Entries |".into(),
                        "|---|---|---|---|---|---|".into(),
                    ]);
                } else {
                    lines.extend(["| Bucket | Hits | Runs |".into(), "|---|---|---|".into()]);
                }
                for b in &e.buckets {
                    let mark = if e.outside.contains(b) {
                        format!(
                            " (outside ODD{}",
                            if e.hits.get(b).copied().unwrap_or(0) != 0 {
                                ", reached)"
                            } else {
                                ")"
                            }
                        )
                    } else if e.is_covered(b) {
                        String::new()
                    } else if e.cover_by == "hits" && e.min_stay.is_none() {
                        " (hole)".into()
                    } else {
                        format!(
                            " (hole: {}/{} {})",
                            fmt_g(e.amount(b)),
                            fmt_g(e.target),
                            unit_of(&e.cover_by)
                        )
                    };
                    let mut row = format!(
                        "| {b}{mark} | {} | {} |",
                        e.hits.get(b).copied().unwrap_or(0),
                        e.runs.get(b).copied().unwrap_or(0)
                    );
                    if timed {
                        let x = e.exposure_of(b);
                        let f = |v: Option<f64>, d: usize| {
                            v.map(|v| format!("{v:.d$}")).unwrap_or_else(|| "?".into())
                        };
                        row += &format!(
                            " {} | {} | {} |",
                            f(x["seconds"], 1),
                            f(x["meters"], 0),
                            f(x["entries"], 0)
                        );
                    }
                    lines.push(row);
                }
                let mut oor: Vec<(&String, &i64)> = e.out_of_range.iter().collect();
                oor.sort();
                for (b, n) in oor {
                    lines.push(format!(
                        "| {b} (outside the buckets) | {n} | - |{}",
                        if timed { " - | - | - |" } else { "" }
                    ));
                }
                if e.samples == 0 {
                    lines.extend([
                        String::new(),
                        "_No samples: the value was never available._".into(),
                    ]);
                }
            }
        }
        lines.extend(cases_markdown(&self.cases, MAX_CASE_ROWS));
        lines.extend(crate::cell_flakiness::markdown(
            &self.cell_flakiness,
            crate::cell_flakiness::MAX_CELL_ROWS,
        ));
        lines.join("\n") + "\n"
    }
}

/// The "Flaky cases" section: the runs that recorded their case, flakiest
/// first (at most *max_rows*).
fn cases_markdown(cases: &[CaseRun], max_rows: usize) -> Vec<String> {
    if cases.is_empty() {
        return Vec::new();
    }
    let failed = |chosen_for: &str| {
        let runs: Vec<&CaseRun> = cases
            .iter()
            .filter(|c| c.chosen_for == chosen_for)
            .collect();
        let failed = runs.iter().filter(|c| c.passed == Some(false)).count();
        (runs.len(), failed)
    };
    let (explored, explored_failed) = failed("instability");
    let (sampled, sampled_failed) = failed("coverage");
    let with_outcome = cases.iter().filter(|c| c.passed.is_some()).count();
    let assessed = cases.iter().filter(|c| c.assessed.is_some()).count();
    let flaky = cases
        .iter()
        .filter(|c| {
            c.assessed
                .is_some_and(|f| f.instability_confidence > FLAKY_CONFIDENCE)
        })
        .count();
    let mut lines = vec![
        String::new(),
        "## Flaky cases".into(),
        String::new(),
        format!(
            "Runs that recorded their case: {} ({with_outcome} with an outcome). \
             Chosen for instability: {explored} ({explored_failed} failed); \
             by the sampler's policy: {sampled} ({sampled_failed} failed).",
            cases.len()
        ),
    ];
    if assessed > 0 {
        lines.push(String::new());
        lines.push(format!(
            "Assessed over all the runs of their scenario: {flaky} of {assessed} flaky \
             (instability confidence above {FLAKY_CONFIDENCE})."
        ));
    }
    let mut order: Vec<&CaseRun> = cases.iter().collect();
    let key = |c: &CaseRun| {
        c.flakiness()
            .map(|f| (f.instability_confidence, f.flip_probability))
            .unwrap_or((-1.0, -1.0))
    };
    order.sort_by(|a, b| {
        let (ka, kb) = (key(a), key(b));
        kb.0.total_cmp(&ka.0).then(kb.1.total_cmp(&ka.1))
    });
    let cell = |f: Option<Flakiness>, pick: fn(&Flakiness) -> Option<f64>| {
        f.as_ref()
            .and_then(pick)
            .map(|x| format!("{x:.2}"))
            .unwrap_or_else(|| "-".into())
    };
    lines.extend([
        String::new(),
        "| Scenario | Case | Chosen for | Passed | Predicted confidence | Confidence | Flip | P(pass) | Radius |".into(),
        "|---|---|---|---|---|---|---|---|---|".into(),
    ]);
    for c in order.iter().take(max_rows) {
        let values = c
            .values
            .iter()
            .map(|(k, v)| match v.as_f64() {
                Some(x) => format!("{k}={}", round3(x)),
                None => format!(
                    "{k}={}",
                    v.as_str().map(String::from).unwrap_or(v.to_string())
                ),
            })
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(format!(
            "| {} | {values} | {} | {} | {} | {} | {} | {} | {} |",
            c.scenario,
            c.chosen_for,
            match c.passed {
                Some(true) => "yes",
                Some(false) => "no",
                None => "-",
            },
            cell(c.predicted, |f| Some(f.instability_confidence)),
            cell(c.assessed, |f| Some(f.instability_confidence)),
            cell(c.assessed, |f| Some(f.flip_probability)),
            cell(c.assessed, |f| Some(f.pass_probability)),
            cell(c.assessed, |f| f.robustness_radius),
        ));
    }
    if cases.len() > max_rows {
        lines.push(format!("\n... ({} more)", cases.len() - max_rows));
    }
    lines
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

fn pct0(x: f64) -> String {
    format!("{:.0}%", x * 100.0)
}

fn pct1(x: f64) -> String {
    format!("{:.1}%", x * 100.0)
}

fn fmt_g(x: f64) -> String {
    crate::value::format_g(x, 6)
}

fn unit_of(cover_by: &str) -> &str {
    match cover_by {
        "seconds" => "s",
        "meters" => "m",
        other => other,
    }
}

fn odd_markdown(e: &OddExposure) -> Vec<String> {
    let total: f64 = e.seconds.values().sum();
    let share = |key: &str| {
        let s = e.seconds[key];
        if total > 0.0 {
            format!("{s:.1} s ({:.1}%)", s / total * 100.0)
        } else {
            format!("{s:.1} s")
        }
    };
    let mut lines = vec![String::new(), format!("## ODD: {}", e.name), String::new()];
    if !e.text.is_empty() {
        lines.extend([e.text.clone(), String::new()]);
    }
    lines.extend([
        format!(
            "- Runs: {}, of which left the ODD: {}",
            e.runs, e.runs_outside
        ),
        format!("- Inside: {}", share("inside")),
        format!(
            "- Inside only because values were missing: {}",
            share("assumed")
        ),
        format!("- Outside: {}", share("outside")),
    ]);
    if !e.unmeasured.is_empty() {
        lines.push(format!(
            "- Monitored but not covered (no buckets): {}",
            e.unmeasured.join(", ")
        ));
    }
    for source in &e.sources {
        let get = |k: &str| {
            source
                .get(k)
                .and_then(Json::as_str)
                .unwrap_or("None")
                .to_string()
        };
        let commit = get("commit");
        lines.push(format!(
            "- Source: {} {} at {} ({})",
            get("git"),
            get("path"),
            get("rev"),
            &commit[..commit.len().min(12)]
        ));
    }
    let mut commits: IndexMap<(String, String), std::collections::BTreeSet<String>> =
        IndexMap::new();
    for s in &e.sources {
        let get = |k: &str| s.get(k).map(|v| v.to_string()).unwrap_or_default();
        commits
            .entry((get("git"), get("path")))
            .or_default()
            .insert(get("commit"));
    }
    if commits.values().any(|c| c.len() > 1) {
        lines.push("- **Warning:** the runs read different commits of a source".into());
    }
    for r in &e.revisions {
        lines.push(format!("- Revision: {r}"));
    }
    if e.revisions.len() > 1 {
        lines.push("- **Warning:** the runs measured different revisions of the ODD".into());
    }
    let failing: Vec<(&String, &IndexMap<String, i64>)> = e
        .module_ticks
        .iter()
        .filter(|(_, c)| c["failed_ticks"] != 0 || c["missing_ticks"] != 0)
        .collect();
    if !failing.is_empty() {
        lines.extend([
            String::new(),
            "| Module | Failed ticks | Ticks on missing values |".into(),
            "|---|---|---|".into(),
        ]);
        for (name, c) in failing {
            lines.push(format!(
                "| {name} | {} | {} |",
                c["failed_ticks"], c["missing_ticks"]
            ));
        }
    }
    if !e.excursions.is_empty() {
        lines.extend([
            String::new(),
            "| Scenario | Outside | First intervals [s] |".into(),
            "|---|---|---|".into(),
        ]);
        let mut worst: Vec<&(String, f64, Json)> = e.excursions.iter().collect();
        worst.sort_by(|a, b| b.1.total_cmp(&a.1));
        for (scenario, seconds, intervals) in worst.into_iter().take(10) {
            let shown: Vec<String> = intervals
                .as_array()
                .into_iter()
                .flatten()
                .take(3)
                .filter_map(|p| {
                    Some(format!(
                        "{}-{}",
                        fmt_g(p.get(0)?.as_f64()?),
                        fmt_g(p.get(1)?.as_f64()?)
                    ))
                })
                .collect();
            lines.push(format!(
                "| {scenario} | {seconds:.1} s | {} |",
                shown.join(", ")
            ));
        }
    }
    lines
}

fn key_of(entry: &Json, kind: &str) -> String {
    let get = |k: &str| entry.get(k).cloned().unwrap_or(Json::Null);
    json!([
        get("name"),
        kind,
        entry.get("event").cloned().unwrap_or(json!("")),
        entry.get("buckets").cloned().unwrap_or(json!([])),
        entry.get("items").cloned().unwrap_or(json!([])),
        entry.get("outside_odd").cloned().unwrap_or(json!([])),
        entry.get("cover_by").cloned().unwrap_or(json!("hits")),
        get("min_stay"),
        entry.get("target").and_then(Json::as_f64).unwrap_or(1.0),
    ])
    .to_string()
}

fn strings(v: Option<&Json>) -> Vec<String> {
    v.and_then(Json::as_array)
        .into_iter()
        .flatten()
        .filter_map(|x| x.as_str().map(String::from))
        .collect()
}

/// Merge coverage documents (the contents of coverage files) into a report.
pub fn merge_coverage(documents: &[Json]) -> Result<CoverageReport> {
    let mut report = CoverageReport::default();
    let mut merged: IndexMap<String, usize> = IndexMap::new();
    let mut by_name: IndexMap<String, usize> = IndexMap::new();
    for (index, doc) in documents.iter().enumerate() {
        let schema = doc.get("schema").and_then(Json::as_str).unwrap_or("");
        if !COVERAGE_SCHEMAS.contains(&schema) {
            return Err(Error::value(format!(
                "not a coverage file of schema {COVERAGE_SCHEMA}: '{schema}'"
            )));
        }
        report.runs += 1;
        let scenario = doc
            .get("scenario")
            .and_then(Json::as_str)
            .unwrap_or("")
            .to_string();
        *report.scenarios.entry(scenario.clone()).or_default() += 1;
        report.cases.extend(CaseRun::read(index, &scenario, doc));
        if let Some(odd) = doc.get("odd").filter(|o| o.is_object()) {
            let name = odd.get("name").and_then(Json::as_str).unwrap_or("");
            report
                .odds
                .entry(name.into())
                .or_insert_with(|| OddExposure::new(name))
                .add(&scenario, odd);
        }
        let mut raws: Vec<(&Json, String)> = Vec::new();
        for e in doc
            .get("items")
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
        {
            raws.push((
                e,
                e.get("kind")
                    .and_then(Json::as_str)
                    .unwrap_or("numeric")
                    .to_string(),
            ));
        }
        for c in doc
            .get("crosses")
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
        {
            raws.push((c, "cross".into()));
        }
        for (raw, kind) in raws {
            let key = key_of(raw, &kind);
            let i = match merged.get(&key) {
                Some(i) => *i,
                None => {
                    let name = raw
                        .get("name")
                        .and_then(Json::as_str)
                        .unwrap_or("")
                        .to_string();
                    let count = by_name.entry(name.clone()).or_default();
                    *count += 1;
                    let buckets = match strings(raw.get("buckets")) {
                        b if !b.is_empty() => b,
                        _ => raw
                            .get("hits")
                            .and_then(Json::as_object)
                            .map(|h| h.keys().cloned().collect())
                            .unwrap_or_default(),
                    };
                    let entry = MergedEntry {
                        name: if *count == 1 {
                            name.clone()
                        } else {
                            format!("{name}#{count}")
                        },
                        group: raw
                            .get("group")
                            .and_then(Json::as_str)
                            .unwrap_or("scenario")
                            .into(),
                        kind: kind.clone(),
                        event: raw.get("event").and_then(Json::as_str).unwrap_or("").into(),
                        text: raw.get("text").and_then(Json::as_str).unwrap_or("").into(),
                        target: raw.get("target").and_then(Json::as_f64).unwrap_or(1.0),
                        cover_by: raw
                            .get("cover_by")
                            .and_then(Json::as_str)
                            .unwrap_or("hits")
                            .into(),
                        min_stay: raw.get("min_stay").and_then(Json::as_f64),
                        buckets,
                        unit: raw.get("unit").and_then(Json::as_str).unwrap_or("").into(),
                        items: strings(raw.get("items")),
                        outside: strings(raw.get("outside_odd")),
                        hits: IndexMap::new(),
                        runs: IndexMap::new(),
                        hit_by: IndexMap::new(),
                        out_of_range: IndexMap::new(),
                        samples: 0,
                        ignored: 0,
                        exposure: IndexMap::new(),
                        counted: IndexMap::new(),
                    };
                    report.entries.push(entry);
                    merged.insert(key, report.entries.len() - 1);
                    report.entries.len() - 1
                }
            };
            let entry = &mut report.entries[i];
            for (bucket, hits) in raw
                .get("hits")
                .and_then(Json::as_object)
                .into_iter()
                .flatten()
            {
                let hits = hits
                    .as_i64()
                    .unwrap_or_else(|| hits.as_f64().unwrap_or(0.0) as i64);
                *entry.hits.entry(bucket.clone()).or_default() += hits;
                if hits != 0 {
                    *entry.runs.entry(bucket.clone()).or_default() += 1;
                    entry.hit_by.entry(bucket.clone()).or_default().push(index);
                }
            }
            for (bucket, hits) in raw
                .get("out_of_range")
                .and_then(Json::as_object)
                .into_iter()
                .flatten()
            {
                *entry.out_of_range.entry(bucket.clone()).or_default() +=
                    hits.as_i64().unwrap_or(0);
            }
            entry.add_exposure(raw);
            entry.samples += raw.get("samples").and_then(Json::as_i64).unwrap_or(0);
            entry.ignored += raw.get("ignored").and_then(Json::as_i64).unwrap_or(0);
        }
    }
    report.cell_flakiness = crate::cell_flakiness::cell_flakiness(documents);
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(hit: &str, seconds: bool) -> Json {
        let mut item = json!({
            "name": "odd.rain", "group": "odd", "kind": "categorical", "event": "tick",
            "buckets": ["none", "heavy"], "target": 1, "outside_odd": ["heavy"], "samples": 10,
            "hits": {"none": if hit == "none" {10} else {0}, "heavy": if hit == "heavy" {10} else {0}},
        });
        if seconds {
            item["seconds"] = json!({"none": 0.5, "heavy": 0.0});
        }
        json!({"schema": COVERAGE_SCHEMA, "scenario": "s", "items": [item]})
    }

    #[test]
    fn runs_merge_and_grade() {
        let r = merge_coverage(&[doc("none", true), doc("none", true)]).unwrap();
        let e = r.entry("odd.rain").unwrap();
        assert_eq!(e.hits["none"], 20);
        assert_eq!(e.runs["none"], 2);
        assert_eq!(e.hit_by["none"], [0, 1]);
        assert_eq!(e.targets(), ["none"]);
        assert_eq!(e.grade(), 1.0);
        assert_eq!(e.exposure_of("none")["seconds"], Some(1.0));
    }

    #[test]
    fn a_run_without_exposure_makes_it_unknown() {
        let r = merge_coverage(&[doc("none", true), doc("none", false)]).unwrap();
        assert_eq!(
            r.entry("odd.rain").unwrap().exposure_of("none")["seconds"],
            None
        );
    }

    #[test]
    fn an_item_defined_differently_is_reported_apart() {
        let mut other = doc("none", true);
        other["items"][0]["buckets"] = json!(["none", "any"]);
        let r = merge_coverage(&[doc("none", true), other]).unwrap();
        let names: Vec<&str> = r.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["odd.rain", "odd.rain#2"]);
    }

    #[test]
    fn markdown_names_the_sources_and_flags_different_commits() {
        let odd = |commit: &str| {
            json!({"name": "calm", "ticks": {"inside": 1}, "seconds": {"inside": 0.1},
            "sources": [{"git": "u", "rev": "main", "commit": commit, "path": "odd.yml"}]})
        };
        let docs = |a: &str, b: &str| {
            vec![
                json!({"schema": COVERAGE_SCHEMA, "scenario": "s1", "items": [], "odd": odd(a)}),
                json!({"schema": COVERAGE_SCHEMA, "scenario": "s2", "items": [], "odd": odd(b)}),
            ]
        };
        let same = merge_coverage(&docs(&"a".repeat(40), &"a".repeat(40)))
            .unwrap()
            .to_markdown(20);
        assert!(
            same.contains("- Source: u odd.yml at main (aaaaaaaaaaaa)"),
            "{same}"
        );
        assert!(!same.contains("different commits"));
        let mixed = merge_coverage(&docs(&"a".repeat(40), &"b".repeat(40)))
            .unwrap()
            .to_markdown(20);
        assert!(
            mixed.contains("the runs read different commits of a source"),
            "{mixed}"
        );
    }

    #[test]
    fn markdown_lists_the_holes() {
        let mut d = doc("heavy", true);
        d["items"][0]["outside_odd"] = json!([]);
        let md = merge_coverage(&[d]).unwrap().to_markdown(20);
        assert!(
            md.contains("| odd.rain | tick | 50% | 1/2 | none |"),
            "{md}"
        );
    }
}
