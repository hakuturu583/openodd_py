//! The ODD matrix: runs and their outcomes over any two ODD attributes.
//!
//! A matrix lays one attribute's buckets along the columns (`x`) and
//! another's along the rows (`y`) and, for each pair of buckets, counts the
//! runs that spent time there, how many of them failed, and how many failed
//! *there*. *Conditions* narrow it: clicking a cell keeps only the time spent
//! in that cell, and the next two attributes are laid out inside it, and so
//! on (`Query::conditions`, outermost first).
//!
//! A run is *in* a cell when it spent at least `min_seconds` there (any step
//! when that is 0). Each cell is one of:
//!
//! | State | Colour | When |
//! |---|---|---|
//! | [`CellState::Outside`] | grey | a bucket of either axis is outside the ODD |
//! | [`CellState::Unsampled`] | white | no run was in it |
//! | [`CellState::Unknown`] | hatched | only runs that recorded no outcome were in it |
//! | [`CellState::Flaky`] | yellow | runs in it both passed and failed |
//! | [`CellState::Pass`] | green | every run in it with an outcome passed |
//! | [`CellState::Fail`] | red | every run in it with an outcome failed |
//!
//! A cell is *tentative* when the evidence is thin: fewer than `min_runs`
//! runs, or both outcomes but an interval reaching outside `band`
//! ([`crate::stats::status`]). The renderer marks it so; its colour still
//! says what the runs did.
//!
//! A run's verdict is the whole run's: a run that failed after leaving a cell
//! still counts as failed in it. `fail_at_cell` counts the runs that were in
//! the cell when they failed (`outcome.failed_cell`), which tells the two
//! apart.

use indexmap::IndexMap;
use serde_json::{json, Value as Json};

use crate::data::{Axis, Dataset, Run};
use crate::stats::{self, CellStatus, BAND, MIN_RUNS};

/// Which runs, and which slice of the ODD, a matrix shows.
#[derive(Debug, Clone, PartialEq)]
pub struct Query {
    /// The column attribute; `None` picks the first attribute not in the
    /// conditions.
    pub x: Option<String>,
    /// The row attribute; `None` picks the next free one; `Some("")` lays
    /// out one row only (a single attribute).
    pub y: Option<String>,
    /// The cells clicked into, outermost first: `(attribute, bucket label)`.
    pub conditions: Vec<(String, String)>,
    /// Only the runs of this scenario (all when `None`).
    pub scenario: Option<String>,
    /// Only the runs of this environment (`Some(None)`: those that recorded
    /// none); all when `None`.
    pub environment: Option<Option<String>>,
    pub min_runs: u64,
    pub band: (f64, f64),
    /// The least a run must stay in a cell to count as in it.
    pub min_seconds: f64,
}

impl Default for Query {
    fn default() -> Self {
        Query {
            x: None,
            y: None,
            conditions: Vec::new(),
            scenario: None,
            environment: None,
            min_runs: MIN_RUNS,
            band: BAND,
            min_seconds: 0.0,
        }
    }
}

impl Query {
    /// A query from JSON: `{"x", "y", "conditions": [[attribute, bucket], ...],
    /// "scenario", "environment", "min_runs", "band": [low, high],
    /// "min_seconds"}`, every key optional. `"environment": null` selects the
    /// runs that recorded none; leaving the key out selects all.
    pub fn from_json(raw: &Json) -> Result<Self, String> {
        let mut q = Query::default();
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
        q.x = text("x")?;
        q.y = text("y")?;
        q.scenario = text("scenario")?;
        if obj.contains_key("environment") {
            q.environment = Some(text("environment")?);
        }
        if let Some(conds) = obj.get("conditions") {
            let list = conds.as_array().ok_or("'conditions' must be a list")?;
            for c in list {
                let pair = c.as_array().filter(|p| p.len() == 2);
                let pair = pair
                    .and_then(|p| Some((p[0].as_str()?.to_string(), p[1].as_str()?.to_string())))
                    .ok_or("a condition is [attribute, bucket]")?;
                q.conditions.push(pair);
            }
        }
        if let Some(n) = obj.get("min_runs") {
            q.min_runs = n
                .as_u64()
                .filter(|n| *n >= 1)
                .ok_or("'min_runs' must be at least 1")?;
        }
        if let Some(b) = obj.get("band") {
            let b = b
                .as_array()
                .filter(|b| b.len() == 2)
                .ok_or("'band' is [low, high]")?;
            let (lo, hi) = (b[0].as_f64(), b[1].as_f64());
            match (lo, hi) {
                (Some(lo), Some(hi)) if (0.0..=1.0).contains(&lo) && lo < hi && hi <= 1.0 => {
                    q.band = (lo, hi)
                }
                _ => return Err("'band' must be (low, high) with 0 <= low < high <= 1".into()),
            }
        }
        if let Some(s) = obj.get("min_seconds") {
            q.min_seconds = s
                .as_f64()
                .filter(|s| s.is_finite() && *s >= 0.0)
                .ok_or("'min_seconds' must be a number of seconds, 0 or more")?;
        }
        Ok(q)
    }
}

/// What a cell's runs did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellState {
    Outside,
    Unsampled,
    Unknown,
    Flaky,
    Pass,
    Fail,
}

impl CellState {
    pub fn as_str(&self) -> &'static str {
        match self {
            CellState::Outside => "outside",
            CellState::Unsampled => "unsampled",
            CellState::Unknown => "unknown",
            CellState::Flaky => "flaky",
            CellState::Pass => "pass",
            CellState::Fail => "fail",
        }
    }
}

/// A cell of the matrix (or the whole of it).
#[derive(Debug, Clone, PartialEq)]
pub struct Cell {
    /// Runs with an outcome that were in it.
    pub runs: u64,
    /// Of those, how many failed, and how many failed while in it.
    pub failed: u64,
    pub fail_at_cell: u64,
    /// Runs without an outcome that were in it.
    pub runs_unknown: u64,
    /// Time and distance all of them spent in it.
    pub seconds: f64,
    pub meters: f64,
    /// Whether a bucket of it is outside the ODD.
    pub outside: bool,
    /// `None` when no run with an outcome was in it.
    pub status: Option<CellStatus>,
    pub state: CellState,
    pub tentative: bool,
}

impl Cell {
    pub fn fail_rate(&self) -> Option<f64> {
        (self.runs > 0).then(|| self.failed as f64 / self.runs as f64)
    }

    /// The 95% Wilson interval of the failure rate.
    pub fn interval(&self) -> Option<(f64, f64)> {
        (self.runs > 0).then(|| stats::wilson(self.failed, self.runs, stats::Z95))
    }

    /// Whether any run was in it.
    pub fn visited(&self) -> bool {
        self.runs + self.runs_unknown > 0
    }

    pub fn to_json(&self) -> Json {
        let interval = self.interval();
        json!({
            "state": self.state.as_str(),
            "tentative": self.tentative,
            "status": self.status.map(|s| s.as_str()),
            "runs": self.runs,
            "failed": self.failed,
            "fail_at_cell": self.fail_at_cell,
            "runs_unknown": self.runs_unknown,
            "fail_rate": self.fail_rate(),
            "ci_low": interval.map(|i| i.0),
            "ci_high": interval.map(|i| i.1),
            "seconds": round3(self.seconds),
            "meters": round3(self.meters),
            "outside_odd": self.outside,
        })
    }
}

fn round3(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

/// A matrix of two attributes (or one), inside some conditions.
#[derive(Debug, Clone, PartialEq)]
pub struct Matrix {
    pub query: Query,
    /// Every attribute, for a host's axis pickers.
    pub attributes: Vec<String>,
    pub x: Option<Axis>,
    pub y: Option<Axis>,
    /// Rows (y's labels, one row without y) x columns (x's labels).
    pub cells: Vec<Vec<Cell>>,
    /// Every run in the conditions, wherever in the matrix.
    pub total: Cell,
    /// Runs selected (scenario, environment), with cells; and documents
    /// without cells, which the matrix cannot place.
    pub runs: usize,
    pub unsampled: usize,
    /// The environments of the selected runs with an outcome: more than one
    /// means runs of different software are pooled.
    pub environments: Vec<Option<String>>,
}

impl Matrix {
    /// The row labels: y's, or one unnamed row.
    pub fn y_labels(&self) -> Vec<String> {
        self.y
            .as_ref()
            .map(|y| y.labels.clone())
            .unwrap_or_else(|| vec![String::new()])
    }

    /// The conditions of the cell at (row, column): this matrix's, then its
    /// row's and column's buckets.
    pub fn conditions_of(&self, row: usize, column: usize) -> Vec<(String, String)> {
        let mut out = self.query.conditions.clone();
        if let Some(y) = &self.y {
            out.push((y.name.clone(), y.labels[row].clone()));
        }
        if let Some(x) = &self.x {
            out.push((x.name.clone(), x.labels[column].clone()));
        }
        out
    }

    /// How many cells (outside the ODD aside) are in each state.
    pub fn counts(&self) -> IndexMap<&'static str, usize> {
        let mut out: IndexMap<&'static str, usize> = [
            CellState::Flaky,
            CellState::Fail,
            CellState::Pass,
            CellState::Unknown,
            CellState::Unsampled,
            CellState::Outside,
        ]
        .iter()
        .map(|s| (s.as_str(), 0))
        .collect();
        for c in self.cells.iter().flatten() {
            *out.get_mut(c.state.as_str()).unwrap() += 1;
        }
        out
    }

    pub fn to_json(&self) -> Json {
        let axis = |a: &Option<Axis>| {
            a.as_ref().map(|a| {
                json!({"name": a.name, "unit": a.unit, "text": a.text,
                       "labels": a.labels, "outside_odd": a.outside})
            })
        };
        json!({
            "x": axis(&self.x),
            "y": axis(&self.y),
            "conditions": self.query.conditions.iter()
                .map(|(a, l)| json!({"attribute": a, "bucket": l})).collect::<Vec<_>>(),
            "scenario": self.query.scenario,
            "environment": self.query.environment,
            "thresholds": {"min_runs": self.query.min_runs, "band": [self.query.band.0, self.query.band.1],
                           "min_seconds": self.query.min_seconds},
            "attributes": self.attributes,
            "cells": self.cells.iter()
                .map(|row| row.iter().map(Cell::to_json).collect::<Vec<_>>()).collect::<Vec<_>>(),
            "total": self.total.to_json(),
            "counts": self.counts(),
            "runs": self.runs,
            "unsampled_runs": self.unsampled,
            "environments": self.environments,
        })
    }
}

/// Per run, what it did in one region: (ticks, seconds, metres, failed there).
#[derive(Default, Clone, Copy)]
struct Stay {
    ticks: u64,
    seconds: f64,
    meters: f64,
    failed_here: bool,
}

#[derive(Default)]
struct Tally {
    runs: u64,
    failed: u64,
    fail_at_cell: u64,
    unknown: u64,
    seconds: f64,
    meters: f64,
}

impl Tally {
    fn add(&mut self, run: &Run, stay: &Stay, min_seconds: f64) {
        if stay.ticks == 0 || stay.seconds < min_seconds {
            return;
        }
        self.seconds += stay.seconds;
        self.meters += stay.meters;
        match run.passed {
            None => self.unknown += 1,
            Some(true) => self.runs += 1,
            Some(false) => {
                self.runs += 1;
                self.failed += 1;
                if stay.failed_here {
                    self.fail_at_cell += 1;
                }
            }
        }
    }

    fn cell(&self, outside: bool, q: &Query) -> Cell {
        let (runs, failed) = (self.runs, self.failed);
        let status = (runs > 0).then(|| stats::status(runs, failed, q.min_runs, q.band));
        let (state, tentative) = if outside {
            (CellState::Outside, false)
        } else if runs == 0 && self.unknown == 0 {
            (CellState::Unsampled, false)
        } else if runs == 0 {
            (CellState::Unknown, false)
        } else {
            let tentative = matches!(status, Some(CellStatus::TooFew | CellStatus::Mixed));
            let state = if failed == 0 {
                CellState::Pass
            } else if failed >= runs {
                CellState::Fail
            } else {
                CellState::Flaky
            };
            (state, tentative)
        };
        Cell {
            runs,
            failed,
            fail_at_cell: self.fail_at_cell,
            runs_unknown: self.unknown,
            seconds: self.seconds,
            meters: self.meters,
            outside,
            status,
            state,
            tentative,
        }
    }
}

impl Dataset {
    /// The runs the query selects (its scenario and environment).
    pub fn selected<'a>(&'a self, q: &'a Query) -> impl Iterator<Item = &'a Run> + 'a {
        self.runs.iter().filter(move |r| {
            q.scenario.as_ref().is_none_or(|s| &r.scenario == s)
                && q.environment.as_ref().is_none_or(|e| &r.environment == e)
        })
    }

    /// The attributes free for an axis inside `conditions`, in order.
    pub fn free_attributes(&self, conditions: &[(String, String)]) -> Vec<String> {
        self.axes
            .keys()
            .filter(|a| !conditions.iter().any(|(c, _)| c == *a))
            .cloned()
            .collect()
    }

    /// The matrix the query asks for. Conditions on attributes no run has
    /// are dropped; an axis on one is not used.
    pub fn matrix(&self, query: &Query) -> Matrix {
        let mut q = query.clone();
        q.conditions.retain(|(a, _)| self.axes.contains_key(a));
        let mut seen = std::collections::HashSet::new();
        q.conditions.retain(|(a, _)| seen.insert(a.clone()));
        let free = self.free_attributes(&q.conditions);
        let pick = |asked: &Option<String>, taken: &[&String]| -> Option<String> {
            match asked {
                Some(a) if a.is_empty() => None,
                Some(a) if free.contains(a) && !taken.contains(&a) => Some(a.clone()),
                _ => free.iter().find(|a| !taken.contains(a)).cloned(),
            }
        };
        let x_name = pick(&q.x, &[]);
        let y_name = match &x_name {
            Some(x) => pick(&q.y, &[x]),
            None => None,
        };
        q.x = x_name.clone();
        q.y = Some(y_name.clone().unwrap_or_default());
        let x = x_name.map(|n| self.axes[&n].clone());
        let y = y_name.map(|n| self.axes[&n].clone());

        let selected: Vec<&Run> = self.selected(&q).collect();
        let mut environments: Vec<Option<String>> = Vec::new();
        for r in selected.iter().filter(|r| r.passed.is_some()) {
            if !environments.contains(&r.environment) {
                environments.push(r.environment.clone());
            }
        }
        let x_labels: Vec<String> = x.as_ref().map(|a| a.labels.clone()).unwrap_or_default();
        let y_labels: Vec<String> = y
            .as_ref()
            .map(|a| a.labels.clone())
            .unwrap_or_else(|| vec![String::new()]);
        let mut grid: Vec<Vec<Tally>> = y_labels
            .iter()
            .map(|_| x_labels.iter().map(|_| Tally::default()).collect())
            .collect();
        let mut total = Tally::default();
        for run in &selected {
            // Where this run's conditions hold: the indices of its labels.
            let conds: Option<Vec<(usize, &String)>> = q
                .conditions
                .iter()
                .map(|(a, l)| run.index_of(a).map(|i| (i, l)))
                .collect();
            let Some(conds) = conds else { continue };
            let xi = x.as_ref().map(|a| run.index_of(&a.name));
            let yi = y.as_ref().map(|a| run.index_of(&a.name));
            if matches!(xi, Some(None)) || matches!(yi, Some(None)) {
                continue; // it did not measure an axis
            }
            let mut stays: IndexMap<(usize, usize), Stay> = IndexMap::new();
            let mut whole = Stay::default();
            for (labels, (ticks, seconds, meters)) in &run.cells {
                if !conds.iter().all(|(i, l)| &labels[*i] == *l) {
                    continue;
                }
                let failed_here = run.failed_cell.as_ref() == Some(labels);
                let column = match xi.flatten() {
                    Some(i) => x_labels.iter().position(|l| *l == labels[i]),
                    None => Some(0),
                };
                let row = match yi.flatten() {
                    Some(i) => y_labels.iter().position(|l| *l == labels[i]),
                    None => Some(0),
                };
                whole.ticks += ticks;
                whole.seconds += seconds;
                whole.meters += meters;
                whole.failed_here |= failed_here;
                if let (Some(r), Some(c)) = (row, column) {
                    let s = stays.entry((r, c)).or_default();
                    s.ticks += ticks;
                    s.seconds += seconds;
                    s.meters += meters;
                    s.failed_here |= failed_here;
                }
            }
            for ((r, c), stay) in &stays {
                grid[*r][*c].add(run, stay, q.min_seconds);
            }
            total.add(run, &whole, q.min_seconds);
        }
        let outside = |row: usize, column: usize| {
            x.as_ref().is_some_and(|a| a.is_outside(&x_labels[column]))
                || y.as_ref().is_some_and(|a| a.is_outside(&y_labels[row]))
        };
        let cells = grid
            .iter()
            .enumerate()
            .map(|(r, row)| {
                row.iter()
                    .enumerate()
                    .map(|(c, t)| t.cell(outside(r, c), &q))
                    .collect()
            })
            .collect();
        let total_outside = q
            .conditions
            .iter()
            .any(|(a, l)| self.axes.get(a).is_some_and(|ax| ax.is_outside(l)));
        Matrix {
            attributes: self.attributes(),
            x,
            y,
            cells,
            total: total.cell(total_outside, &q),
            runs: selected.len(),
            unsampled: self.unsampled,
            environments,
            query: q,
        }
    }
}
