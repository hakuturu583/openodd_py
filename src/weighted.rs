//! Weighted outcomes: the naturalistic failure rate of importance-sampled runs.
//!
//! A run sampled with importance sampling -- NADE's adversarial traffic, say,
//! which makes rare adversities happen far more often than they naturally do
//! -- records its likelihood ratio as `outcome.weight`: the product, over the
//! run, of `p / q` for what was sampled and `(1 - p) / (1 - q)` for what was
//! not. Over `n` such runs, `w_i` their weights and `f_i` 1 for a failure, 0
//! for a pass,
//!
//! * `failure_rate = (1 / n) Σ w_i f_i` is the unbiased estimate of the
//!   *naturalistic* failure rate (the one the runs would show unsampled);
//! * its 95% interval is the normal one, `failure_rate ± z s / √n` with `s²`
//!   the sample variance of `w_i f_i`, clipped to `[0, 1]`;
//! * `relative_half_width = z s / (√n failure_rate)` is how far it has
//!   converged (NADE runs until it falls below a threshold, 0.3 in TeraSim);
//!   a single run has no variance estimate, so its interval is `[0, 1]` and
//!   its relative half width unknown;
//! * `self_normalised_rate = Σ w_i f_i / Σ w_i`, the other common estimate,
//!   biased but of lower variance when the weights vary a lot;
//! * `effective_runs = (Σ w_i)² / Σ w_i²` (Kish's), how many unweighted runs
//!   the weighted ones are worth.
//!
//! Runs are grouped by scenario and environment (`outcome.environment`), as
//! the per-cell outcomes are. A group is reported only when one of its runs
//! recorded a weight; its runs without one count with weight 1. Runs without
//! an outcome, or with a weight that is not a finite non-negative number (the
//! collector writes an invalid one as a string), are left out.

use indexmap::IndexMap;
use serde_json::{json, Value as Json};

use crate::cell_flakiness::Z95;

/// One scenario and environment's weighted outcomes.
#[derive(Debug, Clone, PartialEq)]
pub struct WeightedOutcome {
    pub scenario: String,
    pub environment: Option<String>,
    /// Runs with an outcome.
    pub runs: u64,
    /// Of those, how many failed.
    pub runs_failed: u64,
    /// `Σ w_i`.
    pub weight_sum: f64,
    /// The unbiased estimate of the naturalistic failure rate.
    pub failure_rate: f64,
    /// Its 95% normal interval, clipped to `[0, 1]`; `[0, 1]` for one run.
    pub ci_low: f64,
    pub ci_high: f64,
    /// The interval's half width over the rate; `None` while no run failed
    /// or there is only one run.
    pub relative_half_width: Option<f64>,
    /// `Σ w_i f_i / Σ w_i`; `None` when the weights sum to 0.
    pub self_normalised_rate: Option<f64>,
    /// Kish's effective number of runs.
    pub effective_runs: f64,
    /// The coverage files of the group, by their index in the merge.
    pub documents: Vec<usize>,
}

impl WeightedOutcome {
    pub fn to_json(&self) -> Json {
        json!({
            "scenario": self.scenario,
            "environment": self.environment,
            "runs": self.runs,
            "runs_failed": self.runs_failed,
            "weight_sum": self.weight_sum,
            "failure_rate": self.failure_rate,
            "ci_low": self.ci_low,
            "ci_high": self.ci_high,
            "relative_half_width": self.relative_half_width,
            "self_normalised_rate": self.self_normalised_rate,
            "effective_runs": self.effective_runs,
            "documents": self.documents,
        })
    }
}

/// What one document holds of its outcome.
struct Run {
    failed: bool,
    weight: f64,
    weighted: bool,
    environment: Option<String>,
}

impl Run {
    fn read(doc: &Json) -> Option<Self> {
        let outcome = doc.get("outcome").filter(|o| o.is_object())?;
        let passed = outcome.get("passed").and_then(Json::as_bool)?;
        let (weight, weighted) = match outcome.get("weight") {
            None | Some(Json::Null) => (1.0, false),
            Some(w) => (w.as_f64().filter(|w| w.is_finite() && *w >= 0.0)?, true),
        };
        Some(Run {
            failed: !passed,
            weight,
            weighted,
            environment: match outcome.get("environment") {
                None | Some(Json::Null) => None,
                Some(Json::String(e)) => Some(e.clone()),
                Some(other) => Some(other.to_string()),
            },
        })
    }
}

/// The weighted outcomes of `documents` (the coverage files merged, in
/// order), per scenario and environment in the order the groups first
/// appear; only the groups with a weighted run.
pub fn weighted_outcomes(documents: &[Json]) -> Vec<WeightedOutcome> {
    type Key = (String, Option<String>);
    let mut groups: IndexMap<Key, (Vec<Run>, Vec<usize>)> = IndexMap::new();
    for (index, doc) in documents.iter().enumerate() {
        let Some(run) = Run::read(doc) else {
            continue;
        };
        let scenario = doc
            .get("scenario")
            .and_then(Json::as_str)
            .unwrap_or("")
            .to_string();
        let slot = groups
            .entry((scenario, run.environment.clone()))
            .or_default();
        slot.0.push(run);
        slot.1.push(index);
    }
    groups
        .into_iter()
        .filter(|(_, (runs, _))| runs.iter().any(|r| r.weighted))
        .map(|((scenario, environment), (runs, documents))| {
            estimate(scenario, environment, &runs, documents, Z95)
        })
        .collect()
}

fn estimate(
    scenario: String,
    environment: Option<String>,
    runs: &[Run],
    documents: Vec<usize>,
    z: f64,
) -> WeightedOutcome {
    let n = runs.len() as f64;
    // Second moments are taken of the weights over their largest: a weight
    // is a product over a run and can be small enough for its square to
    // underflow (Kish's ratio does not change with the scale; the variance
    // scales back by its square).
    let scale = runs.iter().map(|r| r.weight).fold(0.0, f64::max);
    let unit = if scale > 0.0 { scale } else { 1.0 };
    let terms: Vec<f64> = runs
        .iter()
        .map(|r| if r.failed { r.weight / unit } else { 0.0 })
        .collect();
    let weight_sum: f64 = runs.iter().map(|r| r.weight).sum();
    let squares: f64 = runs.iter().map(|r| (r.weight / unit).powi(2)).sum();
    let mean = terms.iter().sum::<f64>() / n;
    let failure_rate = mean * unit;
    // One run says nothing of the variance: its interval stays [0, 1] and
    // its relative half width unknown.
    let half = if runs.len() > 1 {
        let variance = terms.iter().map(|t| (t - mean).powi(2)).sum::<f64>() / (n - 1.0);
        Some(z * (variance / n).sqrt() * unit)
    } else {
        None
    };
    WeightedOutcome {
        scenario,
        environment,
        runs: runs.len() as u64,
        runs_failed: runs.iter().filter(|r| r.failed).count() as u64,
        weight_sum,
        failure_rate,
        ci_low: half.map_or(0.0, |h| (failure_rate - h).clamp(0.0, 1.0)),
        ci_high: half.map_or(1.0, |h| (failure_rate + h).clamp(0.0, 1.0)),
        relative_half_width: half
            .filter(|_| failure_rate > 0.0)
            .map(|h| h / failure_rate),
        self_normalised_rate: (weight_sum > 0.0).then(|| mean * unit * n / weight_sum),
        effective_runs: if squares > 0.0 {
            (weight_sum / unit).powi(2) / squares
        } else {
            0.0
        },
        documents,
    }
}

/// The report's section on them; nothing when there is none.
pub fn markdown(outcomes: &[WeightedOutcome]) -> Vec<String> {
    if outcomes.is_empty() {
        return Vec::new();
    }
    let mut lines = vec![
        String::new(),
        "## Weighted outcomes (importance sampling)".to_string(),
        String::new(),
        "The naturalistic failure rate, each run weighted by its likelihood ratio \
         (`outcome.weight`), with its 95% interval."
            .to_string(),
        String::new(),
        "| Scenario | Environment | Runs | Failed | Failure rate | 95% CI | Rel. half width | Effective runs |"
            .to_string(),
        "|---|---|---|---|---|---|---|---|".to_string(),
    ];
    for o in outcomes {
        lines.push(format!(
            "| {} | {} | {} | {} | {:.3e} | [{:.3e}, {:.3e}] | {} | {:.1} |",
            o.scenario,
            o.environment.as_deref().unwrap_or("-"),
            o.runs,
            o.runs_failed,
            o.failure_rate,
            o.ci_low,
            o.ci_high,
            o.relative_half_width
                .map_or("-".to_string(), |r| format!("{r:.2}")),
            o.effective_runs,
        ));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(scenario: &str, outcome: Json) -> Json {
        json!({"schema": "openodd.coverage/1", "scenario": scenario, "outcome": outcome})
    }

    #[test]
    fn unweighted_runs_report_nothing() {
        let docs = [
            doc("a", json!({"passed": true})),
            doc("a", json!({"passed": false})),
        ];
        assert!(weighted_outcomes(&docs).is_empty());
    }

    #[test]
    fn the_rate_is_the_mean_of_the_weighted_failures() {
        let docs = [
            doc("a", json!({"passed": false, "weight": 0.01})),
            doc("a", json!({"passed": true, "weight": 2.0})),
            doc("a", json!({"passed": true})), // weight 1
            doc("a", json!({"passed": false, "weight": 0.03})),
        ];
        let o = &weighted_outcomes(&docs)[0];
        assert_eq!((o.runs, o.runs_failed), (4, 2));
        assert!((o.failure_rate - 0.01).abs() < 1e-12);
        assert!((o.weight_sum - 3.04).abs() < 1e-12);
        assert!((o.self_normalised_rate.unwrap() - 0.04 / 3.04).abs() < 1e-12);
        // Terms 0.01, 0, 0, 0.03: mean 0.01, sample variance 2e-4.
        let half = Z95 * (2e-4f64 / 4.0).sqrt();
        assert!((o.ci_high - (0.01 + half)).abs() < 1e-12);
        assert_eq!(o.ci_low, 0.0); // clipped
        assert!((o.relative_half_width.unwrap() - half / 0.01).abs() < 1e-9);
        let kish = 3.04f64.powi(2) / (1e-4 + 4.0 + 1.0 + 9e-4);
        assert!((o.effective_runs - kish).abs() < 1e-9);
        assert_eq!(o.documents, vec![0, 1, 2, 3]);
    }

    #[test]
    fn groups_split_by_scenario_and_environment() {
        let docs = [
            doc(
                "a",
                json!({"passed": true, "weight": 1.0, "environment": "x"}),
            ),
            doc(
                "a",
                json!({"passed": true, "weight": 1.0, "environment": "y"}),
            ),
            doc("b", json!({"passed": true})), // no weight in its group
        ];
        let groups = weighted_outcomes(&docs);
        let keys: Vec<_> = groups
            .iter()
            .map(|g| (g.scenario.as_str(), g.environment.as_deref()))
            .collect();
        assert_eq!(keys, vec![("a", Some("x")), ("a", Some("y"))]);
        assert_eq!(groups[0].relative_half_width, None); // nothing failed
    }

    #[test]
    fn one_run_has_no_interval() {
        let o = &weighted_outcomes(&[doc("a", json!({"passed": false, "weight": 0.5}))])[0];
        assert_eq!(
            (o.ci_low, o.ci_high, o.relative_half_width),
            (0.0, 1.0, None)
        );
    }

    #[test]
    fn tiny_weights_do_not_underflow() {
        let docs = [
            doc("a", json!({"passed": false, "weight": 1e-200})),
            doc("a", json!({"passed": true, "weight": 1e-200})),
        ];
        let o = &weighted_outcomes(&docs)[0];
        assert!((o.effective_runs - 2.0).abs() < 1e-9);
        assert!(o.ci_high > o.failure_rate);
        assert!((o.relative_half_width.unwrap() - Z95 * (0.5f64 / 2.0).sqrt() / 0.5).abs() < 1e-9);
    }

    #[test]
    fn a_bad_weight_leaves_the_run_out() {
        let docs = [
            doc("a", json!({"passed": false, "weight": -1.0})),
            doc("a", json!({"passed": false, "weight": "x"})),
            doc("a", json!({"passed": true, "weight": 0.5})),
        ];
        let o = &weighted_outcomes(&docs)[0];
        assert_eq!((o.runs, o.documents.clone()), (1, vec![2]));
    }

    #[test]
    fn the_markdown_has_a_row_per_group() {
        let docs = [doc("a", json!({"passed": false, "weight": 0.5}))];
        let md = markdown(&weighted_outcomes(&docs)).join("\n");
        assert!(md.contains("## Weighted outcomes"));
        assert!(md.contains("| a | - | 1 | 1 | 5.000e-1 |"));
        assert!(markdown(&[]).is_empty());
    }
}
