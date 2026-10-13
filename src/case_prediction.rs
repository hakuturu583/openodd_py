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

use std::cell::RefCell;
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

    /// Why there is no prediction at all, if there is none.
    pub fn unavailable(&self) -> Option<&str> {
        self.unavailable.as_deref()
    }

    fn measure(&self, values: &IndexMap<String, Value>) -> Option<Metrics> {
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

impl Predictor for CasePredictor {
    fn grid(&self, plot: &Plot) -> Result<Grid, String> {
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
                vx.iter()
                    .map(|xv| {
                        point.insert(x.name.clone(), xv.clone());
                        point.insert(y.name.clone(), yv.clone());
                        self.measure(&point).map(|m| Predicted {
                            pass_probability: m.pass_probability,
                            flip_probability: m.flip_probability,
                            instability_confidence: m.instability_confidence,
                            state: state(&m),
                        })
                    })
                    .collect()
            })
            .collect();
        Ok(Grid {
            x: gx,
            y: gy,
            cells,
            held,
            runs: self.runs,
            flip_threshold: Config::default().threshold,
            flaky_confidence: FLAKY_CONFIDENCE,
        })
    }

    fn pass_probability(&self, case: &Case) -> Option<f64> {
        self.measure(self.cases.get(&case.key)?)
            .map(|m| m.pass_probability)
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
        assert_eq!(row[0].unwrap().state, Verdict::Fail);
        assert_eq!(row[9].unwrap().state, Verdict::Pass);
        assert!(row[0].unwrap().pass_probability < 0.5);
        let c = &plot.cases[0];
        assert!(p.pass_probability(c).unwrap() < 0.5);
        let j = p.grid_json(&plot);
        assert_eq!(j["cells"][0][9]["state"], "pass");
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
