//! Flakiness per ODD cell: how often the runs that drove through a cell failed.
//!
//! A run's coverage file says how long it spent in each *cell* -- one bucket
//! of each of its ODD's attributes (`odd.cells`, [`crate::cells`]) -- and,
//! when it was recorded, whether it passed (`outcome`). Merged, every cell
//! gets the runs that spent any step in it (`runs_visited`), how many of them
//! failed (`runs_failed`), and the failure rate with a 95% Wilson score
//! interval. A failed run that recorded where it failed
//! (`outcome.failed_cell`, the cell of its last sample at or before
//! `outcome.failed_at`) also counts in `fail_at_cell` of that cell, which
//! tells "failed here" from "passed through here and failed elsewhere".
//!
//! Runs are pooled only with runs of the same scenario, the same environment
//! (`outcome.environment`, an opaque ID: runs of different software must not
//! be pooled, or a regression reads as flakiness; runs without one form their
//! own group) and the same list of ODD attributes (cells of different lists
//! are different things). Runs without an outcome, and runs without a cells
//! table, are left out; a run of no steps counts in its group, in no cell.
//!
//! This is not the per-case flakiness of `crate::flaky`: that one is a model
//! over the parameters a run was *chosen* with; this one counts outcomes over
//! the regions of the ODD a run *drove through*.

use indexmap::{IndexMap, IndexSet};
use serde_json::{json, Value as Json};

pub use openodd_coverage_view::stats::{status, wilson, CellStatus, BAND, MIN_RUNS, Z95};

/// The most cells a group lists in the Markdown report.
pub const MAX_CELL_ROWS: usize = 20;

/// One cell, over the runs of a group.
#[derive(Debug, Clone, PartialEq)]
pub struct CellRow {
    /// One label per attribute of the group.
    pub labels: Vec<String>,
    /// Runs (with an outcome) that spent any step in it.
    pub runs_visited: u64,
    /// Of those, how many failed.
    pub runs_failed: u64,
    /// Failed runs that were in it when they failed.
    pub fail_at_cell: u64,
    /// Seconds and metres the runs spent in it.
    pub seconds: f64,
    pub meters: f64,
}

impl CellRow {
    pub fn fail_rate(&self) -> f64 {
        if self.runs_visited == 0 {
            0.0
        } else {
            self.runs_failed as f64 / self.runs_visited as f64
        }
    }

    /// The 95% Wilson interval of the failure rate.
    pub fn interval(&self) -> (f64, f64) {
        wilson(self.runs_failed, self.runs_visited, Z95)
    }

    /// How sure it is that the cell both passes and fails: the lower end of
    /// the interval of the rarer outcome. 0 for a cell that only passes or
    /// only fails; it exceeds `b` exactly when the interval lies inside the
    /// band `(b, 1 - b)`.
    pub fn flakiness(&self) -> f64 {
        let (low, high) = self.interval();
        low.min(1.0 - high)
    }

    pub fn status(&self, min_runs: u64, band: (f64, f64)) -> CellStatus {
        status(self.runs_visited, self.runs_failed, min_runs, band)
    }

    /// `[[labels...], runs_visited, runs_failed, fail_rate, ci_low, ci_high,
    /// fail_at_cell, seconds, meters]`.
    pub fn to_json(&self) -> Json {
        let (low, high) = self.interval();
        json!([
            self.labels,
            self.runs_visited,
            self.runs_failed,
            self.fail_rate(),
            low,
            high,
            self.fail_at_cell,
            round3(self.seconds),
            round3(self.meters),
        ])
    }
}

fn round3(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

/// The cells of the runs of one scenario, environment and attribute list.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CellFlakiness {
    pub scenario: String,
    /// The runs' environment; `None` for runs that recorded none.
    pub environment: Option<String>,
    /// The attributes a cell's labels are of, in order.
    pub attributes: Vec<String>,
    /// The documents pooled (their indices in the merge).
    pub documents: Vec<usize>,
    /// Of those, how many failed.
    pub runs_failed: u64,
    /// Failed runs that recorded the cell they failed in.
    pub failures_located: u64,
    /// The cells visited, in the order of [`CellFlakiness::sort`].
    pub rows: Vec<CellRow>,
}

impl CellFlakiness {
    pub fn runs(&self) -> u64 {
        self.documents.len() as u64
    }

    /// By status with the default thresholds ([`CellStatus::rank`]: flaky,
    /// failing, mixed, too few runs, passing), so a table cut short still
    /// shows the cells it counts as flaky or failing; then flakiest first
    /// ([`CellRow::flakiness`]), then the higher failure rate, then by labels.
    fn sort(&mut self) {
        self.rows.sort_by(|a, b| {
            let rank = |r: &CellRow| r.status(MIN_RUNS, BAND).rank();
            rank(a)
                .cmp(&rank(b))
                .then(b.flakiness().total_cmp(&a.flakiness()))
                .then(b.fail_rate().total_cmp(&a.fail_rate()))
                .then_with(|| a.labels.cmp(&b.labels))
        });
    }

    /// The cells that are flaky by `min_runs` and `band`, flakiest first.
    pub fn flaky_cells(&self, min_runs: u64, band: (f64, f64)) -> Vec<&CellRow> {
        self.rows
            .iter()
            .filter(|r| r.status(min_runs, band) == CellStatus::Flaky)
            .collect()
    }

    pub fn to_json(&self) -> Json {
        json!({
            "scenario": self.scenario,
            "environment": self.environment,
            "attributes": self.attributes,
            "runs": self.runs(),
            "runs_failed": self.runs_failed,
            "failures_located": self.failures_located,
            "documents": self.documents,
            "rows": self.rows.iter().map(CellRow::to_json).collect::<Vec<_>>(),
        })
    }
}

/// What one document holds of its cells and outcome, if it has both.
struct RunCells {
    passed: bool,
    environment: Option<String>,
    attributes: Vec<String>,
    /// Cell -> (seconds, metres), the cells it spent any step in.
    cells: IndexMap<Vec<String>, (f64, f64)>,
    failed_cell: Option<Vec<String>>,
}

fn strings(v: &Json) -> Option<Vec<String>> {
    v.as_array()?
        .iter()
        .map(|x| x.as_str().map(String::from))
        .collect()
}

impl RunCells {
    fn read(doc: &Json) -> Option<Self> {
        let outcome = doc.get("outcome").filter(|o| o.is_object())?;
        let passed = outcome.get("passed").and_then(Json::as_bool)?;
        let table = doc.pointer("/odd/cells").filter(|c| c.is_object())?;
        let attributes = strings(table.get("attributes")?)?;
        let mut cells: IndexMap<Vec<String>, (f64, f64)> = IndexMap::new();
        for row in table
            .get("rows")
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
        {
            let Some(labels) = row.get(0).and_then(strings) else {
                continue;
            };
            let ticks = row.get(1).and_then(Json::as_f64).unwrap_or(0.0);
            if labels.len() != attributes.len() || ticks <= 0.0 {
                continue; // not a cell of these attributes, or no step in it
            }
            let num = |i: usize| row.get(i).and_then(Json::as_f64).unwrap_or(0.0).max(0.0);
            let slot = cells.entry(labels).or_insert((0.0, 0.0));
            slot.0 += num(2);
            slot.1 += num(3);
        }
        let failed_cell = if passed {
            None
        } else {
            outcome
                .get("failed_cell")
                .and_then(strings)
                .filter(|c| cells.contains_key(c))
        };
        Some(RunCells {
            passed,
            // Another writer's number or bool is still an environment of its
            // own, never pooled with the runs that recorded none.
            environment: match outcome.get("environment") {
                None | Some(Json::Null) => None,
                Some(Json::String(e)) => Some(e.clone()),
                Some(other) => Some(other.to_string()),
            },
            attributes,
            cells,
            failed_cell,
        })
    }
}

/// The cells of `documents` (the coverage files merged, in order), grouped by
/// scenario, environment and attribute list, in the order the groups first
/// appear.
pub fn cell_flakiness(documents: &[Json]) -> Vec<CellFlakiness> {
    type Key = (String, Option<String>, Vec<String>);
    let mut groups: IndexMap<Key, (CellFlakiness, IndexMap<Vec<String>, CellRow>)> =
        IndexMap::new();
    for (index, doc) in documents.iter().enumerate() {
        let Some(run) = RunCells::read(doc) else {
            continue;
        };
        let scenario = doc
            .get("scenario")
            .and_then(Json::as_str)
            .unwrap_or("")
            .to_string();
        let key = (
            scenario.clone(),
            run.environment.clone(),
            run.attributes.clone(),
        );
        let (group, rows) = groups.entry(key).or_insert_with(|| {
            (
                CellFlakiness {
                    scenario,
                    environment: run.environment.clone(),
                    attributes: run.attributes.clone(),
                    ..Default::default()
                },
                IndexMap::new(),
            )
        });
        group.documents.push(index);
        if !run.passed {
            group.runs_failed += 1;
            if run.failed_cell.is_some() {
                group.failures_located += 1;
            }
        }
        for (labels, (seconds, meters)) in run.cells {
            let row = rows.entry(labels.clone()).or_insert_with(|| CellRow {
                labels: labels.clone(),
                runs_visited: 0,
                runs_failed: 0,
                fail_at_cell: 0,
                seconds: 0.0,
                meters: 0.0,
            });
            row.runs_visited += 1;
            row.seconds += seconds;
            row.meters += meters;
            if !run.passed {
                row.runs_failed += 1;
                if run.failed_cell.as_ref() == Some(&labels) {
                    row.fail_at_cell += 1;
                }
            }
        }
    }
    groups
        .into_values()
        .map(|(mut group, rows)| {
            group.rows = rows.into_values().collect();
            group.sort();
            group
        })
        .collect()
}

/// The (scenario, environment) pairs whose runs measured more than one list
/// of attributes, each with the lists' documents.
pub fn mismatches(groups: &[CellFlakiness]) -> Vec<(String, Option<String>, Vec<Vec<usize>>)> {
    let mut by: IndexMap<(String, Option<String>), Vec<Vec<usize>>> = IndexMap::new();
    for g in groups {
        by.entry((g.scenario.clone(), g.environment.clone()))
            .or_default()
            .push(g.documents.clone());
    }
    by.into_iter()
        .filter(|(_, lists)| lists.len() > 1)
        .map(|((s, e), lists)| (s, e, lists))
        .collect()
}

fn pct(x: f64) -> String {
    format!("{:.0}%", x * 100.0)
}

fn fmt_g(x: f64) -> String {
    crate::value::format_g(x, 6)
}

/// The "Flakiness per ODD cell" section of the Markdown report (nothing when
/// no run has both cells and an outcome).
pub fn markdown(groups: &[CellFlakiness], max_rows: usize) -> Vec<String> {
    if groups.is_empty() {
        return Vec::new();
    }
    let (lo, hi) = BAND;
    let mut lines = vec![
        String::new(),
        "## Flakiness per ODD cell".into(),
        String::new(),
        format!(
            "Per cell, the runs that spent any step in it, how many failed, and the failure \
             rate with its 95% Wilson interval. A cell is flaky when at least {MIN_RUNS} runs \
             visited it and the interval lies inside ({}, {}); \"failing\" when every one of \
             them failed. \"Failed here\" counts the failed runs that were in the cell when \
             they failed. Runs are pooled per scenario and environment only.",
            fmt_g(lo),
            fmt_g(hi)
        ),
    ];
    for (scenario, environment, lists) in mismatches(groups) {
        let lists: Vec<String> = lists
            .iter()
            .map(|docs| {
                format!(
                    "[{}]",
                    docs.iter()
                        .map(usize::to_string)
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })
            .collect();
        lines.extend([
            String::new(),
            format!(
                "- **Warning:** the runs of {scenario} ({}) measured different ODD attributes; \
                 each list is reported apart (documents {}).",
                environment.as_deref().unwrap_or("no environment"),
                lists.join(" / ")
            ),
        ]);
    }
    let show_attributes = {
        let mut seen: IndexSet<(&str, Option<&str>)> = IndexSet::new();
        groups
            .iter()
            .any(|g| !seen.insert((&g.scenario, g.environment.as_deref())))
    };
    for g in groups {
        let flaky = g
            .rows
            .iter()
            .filter(|r| r.status(MIN_RUNS, BAND) == CellStatus::Flaky)
            .count();
        let failing = g
            .rows
            .iter()
            .filter(|r| r.status(MIN_RUNS, BAND) == CellStatus::Failing)
            .count();
        let mut title = format!(
            "### {} ({})",
            g.scenario,
            g.environment.as_deref().unwrap_or("no environment")
        );
        if show_attributes {
            title += &format!(" [{}]", g.attributes.join(", "));
        }
        lines.extend([
            String::new(),
            title,
            String::new(),
            format!(
                "Runs: {} ({} failed, {} of them located in a cell). Cells visited: {}; \
                 flaky: {flaky}; always failing: {failing}.",
                g.runs(),
                g.runs_failed,
                g.failures_located,
                g.rows.len()
            ),
            String::new(),
            format!(
                "| {} | Status | Runs | Failed | Fail rate | 95% interval | Failed here | Seconds | Meters |",
                if g.attributes.is_empty() {
                    "Cell".to_string()
                } else {
                    g.attributes.join(" x ")
                }
            ),
            "|---|---|---|---|---|---|---|---|---|".into(),
        ]);
        let cell = |labels: &[String]| -> String {
            if labels.is_empty() {
                return "-".into();
            }
            labels
                .iter()
                .map(|l| l.replace('|', "\\|"))
                .collect::<Vec<_>>()
                .join(" x ")
        };
        for r in g.rows.iter().take(max_rows) {
            let (low, high) = r.interval();
            lines.push(format!(
                "| {} | {} | {} | {} | {} | {}-{} | {} | {:.1} | {:.0} |",
                cell(&r.labels),
                r.status(MIN_RUNS, BAND).as_str().replace('_', " "),
                r.runs_visited,
                r.runs_failed,
                pct(r.fail_rate()),
                pct(low),
                pct(high),
                r.fail_at_cell,
                r.seconds,
                r.meters,
            ));
        }
        if g.rows.len() > max_rows {
            lines.push(format!("\n... ({} more)", g.rows.len() - max_rows));
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn wilson_matches_known_values() {
        assert_eq!(wilson(0, 0, Z95), (0.0, 1.0));
        let (l, h) = wilson(1, 3, Z95);
        assert!(close(l, 0.0615) && close(h, 0.7923), "{l} {h}");
        let (l, h) = wilson(0, 10, Z95);
        assert_eq!(l, 0.0);
        assert!(close(h, 0.2775), "{h}");
        let (l, h) = wilson(10, 10, Z95);
        assert!(close(l, 0.7225) && h == 1.0, "{l}");
        let (l, h) = wilson(5, 10, Z95);
        assert!(close(l, 0.2366) && close(h, 0.7634), "{l} {h}");
    }

    #[test]
    fn statuses() {
        assert_eq!(status(2, 1, 3, BAND), CellStatus::TooFew);
        assert_eq!(status(3, 0, 3, BAND), CellStatus::Passing);
        assert_eq!(status(3, 3, 3, BAND), CellStatus::Failing);
        assert_eq!(status(3, 1, 3, BAND), CellStatus::Flaky);
        assert_eq!(status(10, 1, 3, BAND), CellStatus::Mixed); // 1.8% .. 40%
        assert_eq!(status(10, 1, 3, (0.0, 1.0)), CellStatus::Flaky);
    }

    fn run(scenario: &str, env: Option<&str>, passed: bool, cells: Json, at: Option<Json>) -> Json {
        let mut outcome = json!({"passed": passed});
        if let Some(e) = env {
            outcome["environment"] = json!(e);
        }
        if let Some(c) = at {
            outcome["failed_cell"] = c;
        }
        json!({"schema": crate::report::COVERAGE_SCHEMA, "scenario": scenario, "items": [],
               "odd": {"name": "o", "cells": cells}, "outcome": outcome})
    }

    fn cells(rows: Json) -> Json {
        json!({"attributes": ["speed", "rain"], "rows": rows})
    }

    #[test]
    fn counts_rates_and_locations() {
        let a = json!([["[0, 30)", "none"], 10, 0.5, 4.0]);
        let b = json!([["[30, 60]", "heavy"], 4, 0.2, 2.0]);
        let docs = vec![
            run("s", None, true, cells(json!([a, b])), None),
            run(
                "s",
                None,
                false,
                cells(json!([a, b])),
                Some(json!(["[30, 60]", "heavy"])),
            ),
            run("s", None, false, cells(json!([a])), None),
            json!({"schema": crate::report::COVERAGE_SCHEMA, "scenario": "s", "items": []}),
        ];
        let groups = cell_flakiness(&docs);
        assert_eq!(groups.len(), 1);
        let g = &groups[0];
        assert_eq!((g.runs(), g.runs_failed, g.failures_located), (3, 2, 1));
        let row = |l: &[&str]| g.rows.iter().find(|r| r.labels == l).unwrap();
        let ra = row(&["[0, 30)", "none"]);
        assert_eq!(
            (ra.runs_visited, ra.runs_failed, ra.fail_at_cell),
            (3, 2, 0)
        );
        assert!(close(ra.seconds, 1.5) && close(ra.meters, 12.0));
        let rb = row(&["[30, 60]", "heavy"]);
        assert_eq!(
            (rb.runs_visited, rb.runs_failed, rb.fail_at_cell),
            (2, 1, 1)
        );
        // 2 of 3 is flaky (20.8% .. 93.9%); 1 of 2 is too few runs, though its
        // interval alone (9.5% .. 90.5%) would rank it flakier.
        assert_eq!(g.rows[0].labels, ["[0, 30)", "none"]);
    }

    #[test]
    fn groups_apart_by_environment_and_attributes() {
        let a = json!([["x", "y"], 1, 0.1, 0.0]);
        let other = json!({"attributes": ["speed"], "rows": [[["x"], 1, 0.1, 0.0]]});
        let docs = vec![
            run("s", Some("e1"), true, cells(json!([a])), None),
            run("s", Some("e2"), false, cells(json!([a])), None),
            run("s", None, false, cells(json!([a])), None),
            run("s", Some("e1"), false, other, None),
        ];
        let groups = cell_flakiness(&docs);
        let keys: Vec<(Option<&str>, usize)> = groups
            .iter()
            .map(|g| (g.environment.as_deref(), g.attributes.len()))
            .collect();
        assert_eq!(
            keys,
            [(Some("e1"), 2), (Some("e2"), 2), (None, 2), (Some("e1"), 1)]
        );
        let m = mismatches(&groups);
        assert_eq!(
            m,
            [(
                "s".to_string(),
                Some("e1".to_string()),
                vec![vec![0], vec![3]]
            )]
        );
        assert!(markdown(&groups, 20)
            .join("\n")
            .contains("measured different ODD attributes"));
    }

    #[test]
    fn flaky_and_failing_cells_sort_before_the_rest() {
        // F: 1 of 3 failed (flaky); B: 3 of 3 (failing); T0..T24: 1 of 2
        // each (too few runs, but flakier by the interval alone). The cells
        // the table counts must not be cut off by the 20-row limit.
        let r = |l: &str| json!([[l, "x"], 1, 0.1, 0.0]);
        let ts = || (0..25).map(|k| r(&format!("T{k}"))).collect::<Vec<_>>();
        let with = |mut rows: Vec<Json>, more: Vec<Json>| {
            rows.extend(more);
            cells(json!(rows))
        };
        let docs = vec![
            run("s", None, true, with(vec![r("F")], ts()), None),
            run("s", None, false, with(vec![r("F"), r("B")], ts()), None),
            run("s", None, false, cells(json!([r("B")])), None),
            run("s", None, false, cells(json!([r("B")])), None),
            run("s", None, true, cells(json!([r("F")])), None),
        ];
        let g = &cell_flakiness(&docs)[0];
        let status = |i: usize| g.rows[i].status(MIN_RUNS, BAND);
        assert_eq!(
            (g.rows[0].labels[0].as_str(), status(0)),
            ("F", CellStatus::Flaky)
        );
        assert_eq!(
            (g.rows[1].labels[0].as_str(), status(1)),
            ("B", CellStatus::Failing)
        );
        assert_eq!(status(2), CellStatus::TooFew);
        let md = markdown(&cell_flakiness(&docs), 20).join("\n");
        assert!(
            md.contains("| F x x | flaky |") && md.contains("| B x x | failing |"),
            "{md}"
        );
    }

    #[test]
    fn an_environment_that_is_not_a_string_is_still_one() {
        let mut a = run(
            "s",
            None,
            true,
            cells(json!([[["a", "b"], 1, 0.1, 0.0]])),
            None,
        );
        a["outcome"]["environment"] = json!(123);
        let b = run(
            "s",
            None,
            false,
            cells(json!([[["a", "b"], 1, 0.1, 0.0]])),
            None,
        );
        let groups = cell_flakiness(&[a, b]);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].environment.as_deref(), Some("123"));
    }

    #[test]
    fn a_failure_outside_the_visited_cells_is_not_located() {
        let docs = vec![run(
            "s",
            None,
            false,
            cells(json!([
                [["a", "b"], 3, 0.1, 0.0],
                [["c", "d"], 0, 0.0, 0.0]
            ])),
            Some(json!(["c", "d"])),
        )];
        let g = &cell_flakiness(&docs)[0];
        assert_eq!(g.failures_located, 0);
        assert_eq!(g.rows.len(), 1); // a zero-step row is no visit
    }
}
