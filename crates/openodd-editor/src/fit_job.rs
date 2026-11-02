//! A fit as a job: what the page sends the fit's Web Worker
//! (`src/bin/fit_worker.rs`) and what the worker sends back, as JSON, and
//! the work itself -- so the page can also run it on its own thread when no
//! worker starts, and tests run it natively.

use indexmap::IndexMap;
use openodd::rules::{Given, Origin, RuleSet};
use openodd::rules_fit::{observations_from_json, FitOptions, QuantityFit};
use serde::{Deserialize, Serialize};

use crate::rules_doc;

/// What a fit needs: the rules as the editor writes them, the files they
/// extend, and the data.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Job {
    pub yaml: String,
    pub name: String,
    pub library: Vec<(String, String)>,
    /// The data file's JSON.
    pub data: String,
    pub min_spread: f64,
}

/// One part of a quantity's fit.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PartReport {
    pub name: String,
    pub points: usize,
    pub points_with_data: usize,
    pub estimated: usize,
}

/// One quantity's fit ([`QuantityFit`] in JSON: a log-likelihood of `-inf`
/// is `None`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QuantityReport {
    pub name: String,
    pub rows: usize,
    pub skipped: usize,
    pub log_likelihood_before: Option<f64>,
    pub log_likelihood: Option<f64>,
    pub iterations: usize,
    pub converged: bool,
    pub parts: Vec<PartReport>,
}

impl From<&QuantityFit> for QuantityReport {
    fn from(q: &QuantityFit) -> Self {
        let finite = |v: f64| v.is_finite().then_some(v);
        QuantityReport {
            name: q.name.clone(),
            rows: q.rows,
            skipped: q.skipped,
            log_likelihood_before: finite(q.log_likelihood_before),
            log_likelihood: finite(q.log_likelihood),
            iterations: q.iterations,
            converged: q.converged,
            parts: q
                .parts
                .iter()
                .map(|p| PartReport {
                    name: p.name.clone(),
                    points: p.points,
                    points_with_data: p.points_with_data,
                    estimated: p.estimated,
                })
                .collect(),
        }
    }
}

/// What the worker says.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Reply {
    /// How far it has come ([`openodd::rules_fit::Progress`]).
    Progress {
        fraction: f64,
        quantity: String,
        index: usize,
        count: usize,
    },
    /// The fitted parts as a rule file to merge over the rules, and the report.
    Done {
        overlay: String,
        report: Vec<QuantityReport>,
    },
    Failed {
        error: String,
    },
}

/// Run `job`, telling `progress` how far it has come; the last reply is
/// [`Reply::Done`] or [`Reply::Failed`].
pub fn run(job: &Job, progress: &mut dyn FnMut(Reply)) -> Reply {
    let set = match rules_doc::check(&job.yaml, &job.name, &job.library) {
        Ok(s) => s,
        Err(e) => return Reply::Failed { error: e },
    };
    let obs = match observations_from_json(&job.data, &set.data_names()) {
        Ok(o) => o,
        Err(e) => {
            return Reply::Failed {
                error: format!("the data: {e}"),
            }
        }
    };
    let options = FitOptions {
        min_spread: job.min_spread,
        ..FitOptions::default()
    };
    let mut tell = |p: &openodd::rules_fit::Progress| {
        progress(Reply::Progress {
            fraction: p.fraction,
            quantity: p.quantity.clone(),
            index: p.index,
            count: p.count,
        })
    };
    match set.fit_with_progress(&obs.rows, obs.weights.as_deref(), &options, &mut tell) {
        Ok(fit) => Reply::Done {
            overlay: fit.overlay,
            report: fit.quantities.iter().map(QuantityReport::from).collect(),
        },
        Err(e) => Reply::Failed {
            error: e.to_string(),
        },
    }
}

/// The fitted rules: `overlay` (a [`Reply::Done`]'s) merged over `set`.
pub fn fitted(set: &RuleSet, overlay: &str) -> Result<RuleSet, String> {
    set.parse_over(
        overlay,
        "<fitted rules>",
        &Origin::Text,
        &mut Given(IndexMap::new()),
    )
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_job_runs_and_its_replies_travel_as_json() {
        let yaml = "version: 2\nquantities:\n  grip:\n    parts:\n      water:\n        at: {water: [0, 100]}\n        normal: {mean: [0.5, 0.5], sd: 0.3, low: 0, high: 2}\n";
        let rows: Vec<String> = (0..300)
            .map(|i| {
                let w = (i % 101) as f64;
                format!(
                    r#"{{"water": {w}, "grip": {}}}"#,
                    0.9 - 0.004 * w + 0.01 * ((i % 7) as f64 - 3.0)
                )
            })
            .collect();
        let job = Job {
            yaml: yaml.into(),
            name: "g.yaml".into(),
            library: vec![],
            data: format!("[{}]", rows.join(",")),
            min_spread: 1e-3,
        };
        let sent: Job = serde_json::from_str(&serde_json::to_string(&job).unwrap()).unwrap();
        assert_eq!(sent, job);
        let mut seen = vec![];
        let last = run(&sent, &mut |r| seen.push(r));
        assert!(matches!(seen.last(), Some(Reply::Progress { fraction, .. }) if *fraction == 1.0));
        let text = serde_json::to_string(&last).unwrap();
        assert!(text.starts_with(r#"{"kind":"done""#), "{text}");
        let Reply::Done { overlay, report } = serde_json::from_str(&text).unwrap() else {
            panic!("{text}")
        };
        assert_eq!(report[0].rows, 300);
        let set = rules_doc::check(yaml, "g.yaml", &[]).unwrap();
        let fit = fitted(&set, &overlay).unwrap();
        let mean = &fit.quantities["grip"].parts[0].values[0];
        assert!(
            (mean[0] - 0.9).abs() < 0.02 && (mean[1] - 0.5).abs() < 0.02,
            "{mean:?}"
        );
        let bad = Job {
            data: "[{\"grip\": \"wet\", \"water\": 1}]".into(),
            ..job
        };
        assert!(
            matches!(run(&bad, &mut |_| {}), Reply::Failed { error } if error.contains("not a number"))
        );
    }
}
