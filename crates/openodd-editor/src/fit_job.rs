//! A fit as a job: what the page sends the fit's Web Worker
//! (`src/bin/fit_worker.rs`) and what the worker sends back, as JSON, and
//! the work itself -- so the page can also run it on its own thread when no
//! worker starts, and tests run it natively.

use openodd::rules::RuleSet;
use openodd::rules_fit::{observations_from_json, FitOptions, Observations, Progress, QuantityFit};
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

/// What the worker says.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Reply {
    /// It runs, and takes a job: the page sends it one only then, so an
    /// error before this means no worker started.
    Ready,
    Progress(Progress),
    /// The fitted parts as a rule file to merge over the rules
    /// (`openodd::rules_fit::Fit::text`), and the report.
    Done {
        overlay: String,
        report: Vec<QuantityFit>,
    },
    Failed {
        error: String,
    },
}

/// Run `job`, telling `progress` how far it has come; the reply is
/// [`Reply::Done`] or [`Reply::Failed`].
pub fn run(job: &Job, progress: &mut dyn FnMut(&Progress)) -> Reply {
    let set = match rules_doc::check(&job.yaml, &job.name, &job.library) {
        Ok(s) => s,
        Err(e) => return Reply::Failed { error: e },
    };
    match observations_from_json(&job.data, &set) {
        Ok(obs) => run_parsed(&set, &obs, job.min_spread, progress),
        Err(e) => Reply::Failed {
            error: format!("the data: {e}"),
        },
    }
}

/// [`run`] for rules and data already read.
pub fn run_parsed(
    set: &RuleSet,
    obs: &Observations,
    min_spread: f64,
    progress: &mut dyn FnMut(&Progress),
) -> Reply {
    let options = FitOptions {
        min_spread,
        ..FitOptions::default()
    };
    match set.fit_text_with_progress(&obs.rows, obs.weights(), &options, progress) {
        Ok((overlay, report)) => Reply::Done { overlay, report },
        Err(e) => Reply::Failed {
            error: e.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules_doc::RulesDoc;

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
        let last = run(&sent, &mut |p| seen.push(p.clone()));
        assert_eq!(seen.last().map(|p| p.fraction), Some(1.0));
        let text = serde_json::to_string(&last).unwrap();
        assert!(text.starts_with(r#"{"kind":"done""#), "{text}");
        let Reply::Done { overlay, report } = serde_json::from_str(&text).unwrap() else {
            panic!("{text}")
        };
        assert_eq!(report[0].rows, 300);
        let ready = serde_json::to_string(&Reply::Ready).unwrap();
        assert_eq!(ready, r#"{"kind":"ready"}"#);
        let progress = serde_json::to_string(&Reply::Progress(seen[0].clone())).unwrap();
        assert!(
            progress.starts_with(r#"{"kind":"progress","fraction":"#),
            "{progress}"
        );
        // The overlay, written into the document, gives the fitted rules.
        let mut doc = RulesDoc::from_yaml(yaml).unwrap();
        doc.apply_overlay(&overlay).unwrap();
        let fit = rules_doc::check(&doc.to_yaml().unwrap(), "g.yaml", &[]).unwrap();
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
