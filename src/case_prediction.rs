//! The case plot's background: what the flaky model predicts over two
//! attributes of the cases' space.
//!
//! The view crate (`openodd-coverage-view`) draws a [`Grid`] and fits
//! nothing; this module fits the same Gaussian-process classifier
//! [`crate::flaky`] uses -- `FlakySearch`'s, and the merged report's
//! assessment of each run (same default [`Config`], seed `"report"`) -- to
//! the runs a plot shows, and evaluates it on a grid over the plotted axes.
//!
//! * **Which runs:** the cases a plot selects (scenario, environment) that
//!   recorded their space; when they were drawn in different spaces (or
//!   scenarios), the group with the most runs. Every run with an outcome is
//!   an observation, as in the report.
//! * **The grid:** a numeric axis in [`GRID_STEPS`] cells over the plot's
//!   range, each predicted at its middle; a categorical one, one cell per
//!   band.
//! * **The attributes not plotted** are held at the runs' typical value:
//!   the median of a numeric one, the most common label of a categorical one
//!   (the first in the space's order on a tie). The background is therefore
//!   a slice through the space, not an average over it.
//! * **Each cell** carries the pass probability (what the plot's colour
//!   shows: red at 0, yellow at 0.5, green at 1), the flip probability and
//!   the instability confidence. Its **state** (data only) is flaky where the instability confidence (the posterior
//!   probability that a small change flips the outcome more often than
//!   [`Config::threshold`], 0.1) is above [`FLAKY_CONFIDENCE`] (0.5, the
//!   report's rule for a flaky run); else passes where the pass probability
//!   is at least one half, fails where it is below.
//! * **No grid** with fewer than [`MIN_RUNS`] runs, or runs of one outcome
//!   only: there is nothing to tell apart.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};

use indexmap::IndexMap;
use openodd_coverage_view::cases::{AxisKind, Case, CaseAxis, CaseQuery, CaseSet, Plot, Verdict};
use openodd_coverage_view::prediction::{Grid, GridAxis, Predicted, Predictor};
use openodd_coverage_view::stats::MIN_RUNS;
use serde_json::{json, Value as Json};

use crate::flaky::{Config, Dim, FlakinessModel, Metrics, ParamSpace};
use crate::report::FLAKY_CONFIDENCE;
use crate::value::Value;

/// Cells along a numeric axis of the grid.
pub const GRID_STEPS: usize = 48;

/// The seed the model is fitted with: the merged report's, so a case's
/// predicted pass probability is the one the report assesses.
pub const SEED: &str = "report";

/// A recorded value, as the model reads it.
fn value(v: &Json) -> Value {
    match v {
        Json::Bool(b) => Value::Bool(*b),
        Json::Number(n) => n.as_f64().map(Value::Float).unwrap_or(Value::Missing),
        Json::String(s) => Value::Str(s.clone()),
        _ => Value::Missing,
    }
}

fn values(case: &Case) -> IndexMap<String, Value> {
    case.values
        .iter()
        .map(|(k, v)| (k.clone(), value(v)))
        .collect()
}

/// The state of a point, by the module's rule.
pub fn state(m: &Metrics) -> Verdict {
    if m.instability_confidence > FLAKY_CONFIDENCE {
        Verdict::Flaky
    } else if m.pass_probability >= 0.5 {
        Verdict::Pass
    } else {
        Verdict::Fail
    }
}

fn trimmed(x: f64) -> String {
    let s = format!("{x:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" {
        "0".into()
    } else {
        s.to_string()
    }
}

/// The flaky model of the runs a plot shows, as the case plot's predictor.
pub struct CasePredictor {
    model: Option<RefCell<FlakinessModel>>,
    /// The cases it was fitted to, by key.
    cases: HashMap<String, IndexMap<String, Value>>,
    /// The attributes of its space, and each one's typical value.
    typical: IndexMap<String, Value>,
    runs: usize,
    /// Why there is no model.
    unavailable: Option<String>,
    steps: usize,
    /// Whether each cell of a grid gets the full metrics (flip probability,
    /// instability confidence, state), or the pass probability only.
    detail: bool,
    /// The grids given, by detail and axes: a pair and its transpose are
    /// one grid.
    grids: RefCell<HashMap<GridKey, Grid>>,
    /// The cases' pass probabilities, by key.
    passes: RefCell<HashMap<String, Option<f64>>>,
    /// Points measured in full (Monte Carlo metrics): what costs.
    measured: Cell<usize>,
}

/// A grid's detail and its two axes (as JSON: an axis' edges are floats).
type GridKey = (bool, String, String);

/// `g` with its axes swapped.
fn transposed(g: &Grid) -> Grid {
    let rows = g.x.len();
    let cells = (0..rows)
        .map(|i| g.cells.iter().map(|row| row[i]).collect())
        .collect();
    Grid {
        x: g.y.clone(),
        y: g.x.clone(),
        cells,
        ..g.clone()
    }
}

impl CasePredictor {
    /// The model of the cases `query` selects in `set`.
    pub fn new(set: &CaseSet, query: &CaseQuery) -> Self {
        Self::with_steps(set, query, GRID_STEPS)
    }

    pub fn with_steps(set: &CaseSet, query: &CaseQuery, steps: usize) -> Self {
        let mut out = CasePredictor {
            model: None,
            cases: HashMap::new(),
            typical: IndexMap::new(),
            runs: 0,
            unavailable: None,
            steps: steps.max(2),
            detail: true,
            grids: RefCell::new(HashMap::new()),
            passes: RefCell::new(HashMap::new()),
            measured: Cell::new(0),
        };
        // The cases by (scenario, space); the group with the most runs.
        let cases = set.selected(query);
        let mut groups: IndexMap<(String, String), Vec<&Case>> = IndexMap::new();
        for c in &cases {
            if let Some(space) = &c.space {
                groups
                    .entry((c.scenario.clone(), space.to_string()))
                    .or_default()
                    .push(c);
            }
        }
        let runs_of = |g: &Vec<&Case>| g.iter().map(|c| c.runs.len()).sum::<usize>();
        let Some((_, group)) = groups
            .iter()
            .max_by_key(|(k, g)| (runs_of(g), std::cmp::Reverse(k.1.len())))
        else {
            out.unavailable = Some("the cases recorded no space to model them in".into());
            return out;
        };
        let space = match ParamSpace::from_json(group[0].space.as_ref().expect("grouped")) {
            Ok(s) => s,
            Err(e) => {
                out.unavailable = Some(e.to_string());
                return out;
            }
        };
        let mut model = match FlakinessModel::new(space.clone(), Config::default(), SEED) {
            Ok(m) => m,
            Err(e) => {
                out.unavailable = Some(e.to_string());
                return out;
            }
        };
        let (mut passed, mut failed) = (0usize, 0usize);
        let mut observed: Vec<(&Case, IndexMap<String, Value>)> = Vec::new();
        for c in group {
            let v = values(c);
            for r in &c.runs {
                if model.observe(&v, r.passed).is_ok() {
                    if r.passed {
                        passed += 1;
                    } else {
                        failed += 1;
                    }
                }
            }
            if space.encode(&v).is_ok() {
                observed.push((c, v));
            }
        }
        out.runs = passed + failed;
        if (out.runs as u64) < MIN_RUNS {
            out.unavailable = Some(format!(
                "too few runs to predict ({}; at least {MIN_RUNS})",
                out.runs
            ));
            return out;
        }
        if passed == 0 || failed == 0 {
            out.unavailable = Some(format!(
                "every run {}, so there is nothing to tell apart",
                if failed == 0 { "passed" } else { "failed" }
            ));
            return out;
        }
        // Each parameter's typical value over the runs.
        for dim in space.dims() {
            let name = dim.name().to_string();
            let weighted = observed
                .iter()
                .flat_map(|(c, v)| std::iter::repeat_n(v.get(&name), c.runs.len()));
            let typical = match dim {
                Dim::Numeric { .. } => {
                    let mut xs: Vec<f64> = weighted.flatten().filter_map(Value::number).collect();
                    xs.sort_by(f64::total_cmp);
                    if xs.is_empty() {
                        continue;
                    }
                    let n = xs.len();
                    let median = if n % 2 == 1 {
                        xs[n / 2]
                    } else {
                        (xs[n / 2 - 1] + xs[n / 2]) / 2.0
                    };
                    Value::Float(median)
                }
                Dim::Categorical { labels, .. } => {
                    let mut count: HashMap<String, usize> = HashMap::new();
                    for v in weighted.flatten() {
                        *count.entry(v.label()).or_default() += 1;
                    }
                    let best = labels.iter().max_by_key(|l| {
                        (
                            count.get(*l).copied().unwrap_or(0),
                            std::cmp::Reverse(labels.iter().position(|x| x == *l)),
                        )
                    });
                    match best {
                        Some(l) => Value::Str(l.clone()),
                        None => continue,
                    }
                }
            };
            out.typical.insert(name, typical);
        }
        out.cases = observed
            .into_iter()
            .map(|(c, v)| (c.key.clone(), v))
            .collect();
        out.model = Some(RefCell::new(model));
        out
    }

    /// Grids with the pass probability only (`false`): what the plot's
    /// colour needs, at a fraction of the cost -- one posterior mean and
    /// variance per cell instead of 32 neighbours and 256 posterior draws.
    /// The default (`true`) measures every cell fully.
    pub fn with_detail(mut self, detail: bool) -> Self {
        self.detail = detail;
        self
    }

    /// Points measured in full so far (the Monte Carlo metrics of a
    /// detailed grid's cells): the cases' pass probabilities and a plain
    /// grid need none.
    pub fn measured(&self) -> usize {
        self.measured.get()
    }

    /// Why there is no prediction at all, if there is none.
    pub fn unavailable(&self) -> Option<&str> {
        self.unavailable.as_deref()
    }

    /// The pass probability at each point (`None` where a point is outside
    /// the model's space), in one solve: the posterior mean and variance
    /// only, not the Monte Carlo metrics.
    fn pass_only<'a>(
        &self,
        points: impl IntoIterator<Item = &'a IndexMap<String, Value>>,
    ) -> Vec<Option<f64>> {
        let points: Vec<_> = points.into_iter().collect();
        let Some(model) = self.model.as_ref() else {
            return vec![None; points.len()];
        };
        let mut model = model.borrow_mut();
        let coded: Vec<Option<Vec<f64>>> = points
            .iter()
            .map(|v| model.space().encode(v).ok())
            .collect();
        let Ok(classifier) = model.classifier() else {
            return vec![None; points.len()];
        };
        let known: Vec<Vec<f64>> = coded.iter().flatten().cloned().collect();
        let mut p = classifier.pass_probabilities(&known).into_iter();
        coded
            .iter()
            .map(|x| x.as_ref().and_then(|_| p.next()))
            .collect()
    }

    fn measure(&self, values: &IndexMap<String, Value>) -> Option<Metrics> {
        self.measured.set(self.measured.get() + 1);
        let mut model = self.model.as_ref()?.borrow_mut();
        let x = model.space().encode(values).ok()?;
        model.map_metrics_at(&x).ok()
    }

    /// One plotted axis of the grid: its cells and the value at each.
    fn axis(&self, a: &CaseAxis) -> (GridAxis, Vec<Value>) {
        match a.kind {
            AxisKind::Numeric => {
                let (lo, hi) = a.domain();
                let n = self.steps;
                let edges: Vec<f64> = (0..=n)
                    .map(|i| lo + (hi - lo) * i as f64 / n as f64)
                    .collect();
                let mids = edges
                    .windows(2)
                    .map(|w| Value::Float((w[0] + w[1]) / 2.0))
                    .collect();
                (
                    GridAxis::Numeric {
                        name: a.name.clone(),
                        edges,
                    },
                    mids,
                )
            }
            AxisKind::Categorical => (
                GridAxis::Categorical {
                    name: a.name.clone(),
                    labels: a.labels.clone(),
                },
                a.labels.iter().map(|l| Value::Str(l.clone())).collect(),
            ),
        }
    }

    /// The grid as JSON: [`Grid::to_json`], or `{"unavailable": why}`.
    pub fn grid_json(&self, plot: &Plot) -> Json {
        match self.grid(plot) {
            Ok(g) => g.to_json(),
            Err(why) => json!({"unavailable": why}),
        }
    }
}

impl CasePredictor {
    /// [`Predictor::grid`], with the full metrics in each cell (`detail`)
    /// or the pass probability only, whatever [`Self::with_detail`] says.
    /// Grids are kept: asking again, or for the transposed pair, costs
    /// nothing.
    pub fn grid_with(&self, plot: &Plot, detail: bool) -> Result<Grid, String> {
        if let Some(why) = &self.unavailable {
            return Err(why.clone());
        }
        let (Some(x), Some(y)) = (&plot.x, &plot.y) else {
            return Err("it needs two attributes".into());
        };
        for a in [x, y] {
            if !self.typical.contains_key(&a.name) {
                return Err(format!(
                    "{} is not a parameter of the space the cases were drawn in",
                    a.name
                ));
            }
        }
        let (gx, vx) = self.axis(x);
        let (gy, vy) = self.axis(y);
        let (jx, jy) = (gx.to_json().to_string(), gy.to_json().to_string());
        {
            let grids = self.grids.borrow();
            if let Some(g) = grids.get(&(detail, jx.clone(), jy.clone())) {
                return Ok(g.clone());
            }
            if let Some(g) = grids.get(&(detail, jy.clone(), jx.clone())) {
                return Ok(transposed(g));
            }
        }
        let plotted: HashSet<&str> = [x.name.as_str(), y.name.as_str()].into();
        let held: Vec<(String, String)> = self
            .typical
            .iter()
            .filter(|(k, _)| !plotted.contains(k.as_str()))
            .map(|(k, v)| {
                let text = match v.number() {
                    Some(n) if !matches!(v, Value::Str(_)) => trimmed(n),
                    _ => v.label(),
                };
                (k.clone(), text)
            })
            .collect();
        let mut point = self.typical.clone();
        let cells = vy
            .iter()
            .map(|yv| {
                point.insert(y.name.clone(), yv.clone());
                if !detail {
                    // A row in one solve.
                    let row: Vec<_> = vx
                        .iter()
                        .map(|xv| {
                            let mut p = point.clone();
                            p.insert(x.name.clone(), xv.clone());
                            p
                        })
                        .collect();
                    return self
                        .pass_only(&row)
                        .into_iter()
                        .map(|p| {
                            p.map(|p| Predicted {
                                pass_probability: p,
                                flip_probability: None,
                                instability_confidence: None,
                                state: None,
                            })
                        })
                        .collect();
                }
                vx.iter()
                    .map(|xv| {
                        point.insert(x.name.clone(), xv.clone());
                        self.measure(&point).map(|m| Predicted {
                            pass_probability: m.pass_probability,
                            flip_probability: Some(m.flip_probability),
                            instability_confidence: Some(m.instability_confidence),
                            state: Some(state(&m)),
                        })
                    })
                    .collect()
            })
            .collect();
        let grid = Grid {
            x: gx,
            y: gy,
            cells,
            held,
            runs: self.runs,
            flip_threshold: Config::default().threshold,
            flaky_confidence: FLAKY_CONFIDENCE,
        };
        self.grids
            .borrow_mut()
            .insert((detail, jx, jy), grid.clone());
        Ok(grid)
    }
}

impl Predictor for CasePredictor {
    fn grid(&self, plot: &Plot) -> Result<Grid, String> {
        self.grid_with(plot, self.detail)
    }

    /// The posterior pass probability at the case's values -- what the
    /// Monte Carlo metrics' `pass_probability` is, without their cost.
    fn pass_probability(&self, case: &Case) -> Option<f64> {
        self.pass_probabilities(&[case])[0]
    }

    fn pass_probabilities(&self, cases: &[&Case]) -> Vec<Option<f64>> {
        let missing: Vec<&Case> = {
            let known = self.passes.borrow();
            let mut seen = HashSet::new();
            cases
                .iter()
                .filter(|c| !known.contains_key(&c.key) && seen.insert(&c.key))
                .copied()
                .collect()
        };
        if !missing.is_empty() {
            let (asked, unknown): (Vec<&Case>, Vec<&Case>) = missing
                .into_iter()
                .partition(|c| self.cases.contains_key(&c.key));
            let p = self.pass_only(asked.iter().map(|c| &self.cases[&c.key]));
            let mut known = self.passes.borrow_mut();
            for (c, p) in asked.iter().zip(p) {
                known.insert(c.key.clone(), p);
            }
            for c in unknown {
                known.insert(c.key.clone(), None);
            }
        }
        let known = self.passes.borrow();
        cases.iter().map(|c| known[&c.key]).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(gap: f64, rain: &str, passed: bool) -> Json {
        json!({
            "scenario": "s",
            "case": {
                "values": {"gap": gap, "rain": rain, "speed": 10.0},
                "overrides": [format!("gap={gap}"), format!("rain={rain}")],
                "chosen_for": "coverage",
                "space": [{"name": "gap", "range": [0.0, 40.0]},
                          {"name": "rain", "labels": ["none", "heavy"]},
                          {"name": "speed", "range": [0.0, 20.0]}],
            },
            "outcome": {"passed": passed},
        })
    }

    #[test]
    fn the_grid_tells_passes_from_fails() {
        // Passes above a gap of 20 m, whatever the rain.
        let docs: Vec<Json> = (0..16)
            .map(|i| {
                let gap = 2.0 + i as f64 * 2.4;
                doc(gap, if i % 2 == 0 { "none" } else { "heavy" }, gap > 20.0)
            })
            .collect();
        let set = CaseSet::from_documents(&docs);
        let plot = set.plot(&CaseQuery {
            x: Some("gap".into()),
            y: Some("rain".into()),
            ..Default::default()
        });
        let p = CasePredictor::with_steps(&set, &CaseQuery::default(), 10);
        let g = p.grid(&plot).unwrap();
        assert_eq!(g.runs, 16);
        assert_eq!(g.held, vec![("speed".to_string(), "10".to_string())]);
        assert_eq!((g.x.len(), g.y.len()), (10, 2));
        let row = &g.cells[0];
        assert_eq!(row[0].unwrap().state, Some(Verdict::Fail));
        assert_eq!(row[9].unwrap().state, Some(Verdict::Pass));
        // The pass probability alone is the same number, without the rest.
        let fast = CasePredictor::with_steps(&set, &CaseQuery::default(), 10).with_detail(false);
        let f = fast.grid(&plot).unwrap();
        for (a, b) in f.cells.iter().flatten().zip(g.cells.iter().flatten()) {
            let (a, b) = (a.unwrap(), b.unwrap());
            assert!((a.pass_probability - b.pass_probability).abs() < 1e-9);
            assert!(a.state.is_none() && a.flip_probability.is_none());
        }
        assert!(row[0].unwrap().pass_probability < 0.5);
        let c = &plot.cases[0];
        assert!(p.pass_probability(c).unwrap() < 0.5);
        let j = p.grid_json(&plot);
        assert_eq!(j["cells"][0][9]["state"], "pass");
    }

    #[test]
    fn grids_are_kept_and_cases_need_no_monte_carlo() {
        let docs: Vec<Json> = (0..24)
            .map(|i| {
                let gap = 1.0 + i as f64 * 1.6;
                doc(gap, if i % 3 == 0 { "heavy" } else { "none" }, gap > 18.0)
            })
            .collect();
        let set = CaseSet::from_documents(&docs);
        let query = |x: &str, y: &str| CaseQuery {
            x: Some(x.into()),
            y: Some(y.into()),
            ..Default::default()
        };
        let p = CasePredictor::with_steps(&set, &CaseQuery::default(), 6).with_detail(false);
        // The tips and every pair of the explorer: no point measured in full.
        let opts = openodd_coverage_view::case_plot::Options::default();
        let html = openodd_coverage_view::case_plot::explorer_with(
            &set,
            &query("gap", "speed"),
            &opts,
            Some(&p),
        );
        assert!(html.contains("Pass probability (model, now)"));
        assert_eq!(p.measured(), 0);
        // A case's pass probability is the full metrics' one.
        let full = CasePredictor::with_steps(&set, &CaseQuery::default(), 6);
        for c in &set.cases {
            let m = full.measure(&full.cases[&c.key]).unwrap();
            assert!((p.pass_probability(c).unwrap() - m.pass_probability).abs() < 1e-9);
        }
        // A pair and its transpose are one grid, cell for cell.
        let (a, b) = (
            set.plot(&query("gap", "rain")),
            set.plot(&query("rain", "gap")),
        );
        let ga = full.grid(&a).unwrap();
        let measured = full.measured();
        let gb = full.grid(&b).unwrap();
        assert_eq!(
            full.measured(),
            measured,
            "the transpose is not measured again"
        );
        let fresh = CasePredictor::with_steps(&set, &CaseQuery::default(), 6)
            .grid(&b)
            .unwrap();
        assert_eq!(gb, fresh);
        assert_eq!((gb.x.name(), gb.y.name()), ("rain", "gap"));
        assert_eq!(gb.cells[3][0], ga.cells[0][3]);
        // A plain grid row in one solve: the same numbers as one by one.
        let plain = p.grid(&a).unwrap();
        for (r, row) in plain.cells.iter().enumerate() {
            for (c, cell) in row.iter().enumerate() {
                let d = ga.cells[r][c].unwrap().pass_probability;
                assert!((cell.unwrap().pass_probability - d).abs() < 1e-9);
            }
        }
    }

    #[test]
    fn one_outcome_or_too_few_runs_predict_nothing() {
        let docs: Vec<Json> = (0..5).map(|i| doc(i as f64 * 5.0, "none", true)).collect();
        let set = CaseSet::from_documents(&docs);
        let p = CasePredictor::new(&set, &CaseQuery::default());
        assert!(p.unavailable().unwrap().contains("every run passed"));
        let few = CaseSet::from_documents(&docs[..2]);
        let p = CasePredictor::new(&few, &CaseQuery::default());
        assert!(p.unavailable().unwrap().contains("too few runs"));
        let plot = set.plot(&CaseQuery::default());
        assert!(CasePredictor::new(&set, &CaseQuery::default())
            .grid_json(&plot)
            .get("unavailable")
            .is_some());
    }
}
