//! How well a scenario stages what the sampler aims at: each run's drawn
//! values against what its probes measured at its start.
//!
//! A case's `values` are what the sampler *drew*: the ODD values it aimed
//! at. They reach the run through the scenario's knobs (an override, often
//! with an offset and a scale), and the run's probes then *measure* what
//! the scenario really staged (`odd.samples`). When a knob is miscalibrated
//! the two differ, systematically: a constant bias is a wrong offset, an
//! error that grows with the value a wrong scale.
//!
//! Per attribute, over every run with both ([`Drift`]):
//!
//! * a numeric one: each run's `(drawn, measured)`, the bias (mean of
//!   measured - drawn), the mean and largest absolute error, and the share
//!   of runs whose measured value fell in the same ODD bucket as the drawn
//!   one (the *bucket hit rate*);
//! * a categorical one: how often each drawn label was measured as each
//!   label, and the share that agreed.
//!
//! "Measured" is each attribute's first sample within the query's window
//! from t = 0 ([`CaseQuery::window`]).
//!
//! **Calibration.** For a numeric attribute, `measured ≈ a × drawn + b` is
//! fitted by least squares over the runs ([`Fit`]). When the fit can be
//! trusted ([`Calibration`]), writing the drawn value `d` as `(d − b) / a`
//! through the existing knob stages `d`: for a knob that writes
//! `scale × value + offset`, that is a knob of scale `scale / a` and offset
//! `offset − scale × b / a`. A fit is not trusted with fewer than
//! [`MIN_FIT_RUNS`] runs, with `|a|` below [`MIN_SLOPE`] (the measured value
//! does not follow the drawn one), or with a residual above
//! [`MAX_RESIDUAL`] of the attribute's typical bucket width; and when the
//! residual follows another attribute (its share of the residual's variance
//! above [`MAX_STRUCTURE`]), no constant offset and scale can fix it. A fit
//! within [`NEGLIGIBLE`] of the bucket width (or twice its residual) of
//! `measured = drawn` over the drawn range needs no correction.

use indexmap::IndexMap;

use serde_json::{json, Value as Json};

use crate::cases::{value_text, AxisKind, CaseQuery, CaseSet, Span};

/// The fewest runs a calibration is suggested from.
pub const MIN_FIT_RUNS: usize = 5;
/// Below this `|a|`, the measured value does not follow the drawn one.
pub const MIN_SLOPE: f64 = 0.1;
/// The largest residual standard deviation, as a share of the attribute's
/// median bucket width (2% of its range without buckets), a fit is trusted
/// with.
pub const MAX_RESIDUAL: f64 = 0.25;
/// A correction smaller than this share of the bucket width (or twice the
/// residual) is not worth making: the knob is called calibrated.
pub const NEGLIGIBLE: f64 = 0.05;
/// The largest share of the residual's variance another attribute may
/// explain before the drift is called structured.
pub const MAX_STRUCTURE: f64 = 0.5;

/// `measured ≈ a × drawn + b`, by least squares.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fit {
    pub runs: usize,
    pub a: f64,
    pub b: f64,
    /// The residual's standard deviation.
    pub residual_std: f64,
    pub r2: f64,
}

/// What the fit says about the knob that writes the attribute.
#[derive(Debug, Clone, PartialEq)]
pub enum Calibration {
    /// Write the drawn value `d` as `(d − b) / a` through the existing knob.
    Suggest { a: f64, b: f64 },
    /// The fit is within its residual of `measured = drawn`.
    Calibrated,
    /// Too few runs.
    TooFew { runs: usize },
    /// `|a|` near 0: the measured value does not follow the drawn one.
    Flat { a: f64 },
    /// The residual is large against the bucket width.
    Noisy { residual_std: f64, width: f64 },
    /// The residual follows another attribute.
    Structured { attribute: String, share: f64 },
}

impl Calibration {
    pub fn status(&self) -> &'static str {
        match self {
            Calibration::Suggest { .. } => "suggest",
            Calibration::Calibrated => "calibrated",
            Calibration::TooFew { .. } => "too_few",
            Calibration::Flat { .. } => "flat",
            Calibration::Noisy { .. } => "noisy",
            Calibration::Structured { .. } => "structured",
        }
    }
}

/// A knob's corrected `(scale, offset)`, from its current ones and the fit:
/// the knob composed with the fit's inverse.
pub fn corrected_knob(scale: f64, offset: f64, a: f64, b: f64) -> (f64, f64) {
    (scale / a, offset - scale * b / a)
}

/// The share of `y`'s variance that `x` explains: r² for numbers, η² (the
/// between-group share) for labels.
fn explained(y: &[f64], x: &[&Json]) -> Option<f64> {
    let n = y.len() as f64;
    if y.len() < 3 {
        return None;
    }
    let my = y.iter().sum::<f64>() / n;
    let syy: f64 = y.iter().map(|v| (v - my).powi(2)).sum();
    if syy <= 0.0 {
        return None;
    }
    let nums: Vec<f64> = x.iter().filter_map(|v| v.as_f64()).collect();
    if nums.len() == x.len() {
        let mx = nums.iter().sum::<f64>() / n;
        let sxx: f64 = nums.iter().map(|v| (v - mx).powi(2)).sum();
        if sxx <= 0.0 {
            return None;
        }
        let sxy: f64 = nums.iter().zip(y).map(|(a, b)| (a - mx) * (b - my)).sum();
        return Some(sxy * sxy / (sxx * syy));
    }
    let mut groups: IndexMap<String, (f64, usize)> = IndexMap::new();
    for (v, yv) in x.iter().zip(y) {
        let g = groups.entry(value_text(v)).or_insert((0.0, 0));
        g.0 += yv;
        g.1 += 1;
    }
    if groups.len() < 2 {
        return None;
    }
    let between: f64 = groups
        .values()
        .map(|(s, k)| *k as f64 * (s / *k as f64 - my).powi(2))
        .sum();
    // Adjusted for the number of groups: a few runs per label explain some
    // variance by chance.
    let k = groups.len() as f64;
    let eta2 = between / syy;
    Some((eta2 - (k - 1.0) / (n - 1.0) * (1.0 - eta2)).max(0.0))
}

/// One run of a numeric attribute.
#[derive(Debug, Clone, PartialEq)]
pub struct DriftPoint {
    /// The case's key and its number in the selection (1-based).
    pub case: String,
    pub case_number: usize,
    pub document: usize,
    pub passed: bool,
    pub drawn: f64,
    pub measured: f64,
    pub drawn_bucket: String,
    pub measured_bucket: String,
    /// The case's other drawn values: what a structured residual is
    /// looked for in.
    pub others: Vec<(String, Json)>,
}

impl DriftPoint {
    pub fn error(&self) -> f64 {
        self.measured - self.drawn
    }

    pub fn same_bucket(&self) -> bool {
        self.drawn_bucket == self.measured_bucket
    }
}

/// A numeric attribute's drift.
#[derive(Debug, Clone, PartialEq)]
pub struct NumericDrift {
    pub name: String,
    pub unit: String,
    /// Its ODD buckets: where the grid lines go, and what a hit is.
    pub spans: Vec<Span>,
    pub points: Vec<DriftPoint>,
}

impl NumericDrift {
    /// `measured ≈ a × drawn + b` over the runs; `None` with fewer than two
    /// distinct drawn values.
    pub fn fit(&self) -> Option<Fit> {
        let n = self.points.len();
        if n < 2 {
            return None;
        }
        let nf = n as f64;
        let mx = self.points.iter().map(|p| p.drawn).sum::<f64>() / nf;
        let my = self.points.iter().map(|p| p.measured).sum::<f64>() / nf;
        let sxx: f64 = self.points.iter().map(|p| (p.drawn - mx).powi(2)).sum();
        if sxx <= 1e-12 {
            return None;
        }
        let sxy: f64 = self
            .points
            .iter()
            .map(|p| (p.drawn - mx) * (p.measured - my))
            .sum();
        let a = sxy / sxx;
        let b = my - a * mx;
        let ss_res: f64 = self
            .points
            .iter()
            .map(|p| (p.measured - (a * p.drawn + b)).powi(2))
            .sum();
        let ss_tot: f64 = self.points.iter().map(|p| (p.measured - my).powi(2)).sum();
        Some(Fit {
            runs: n,
            a,
            b,
            residual_std: (ss_res / (nf - 2.0).max(1.0)).sqrt(),
            r2: if ss_tot > 0.0 {
                1.0 - ss_res / ss_tot
            } else {
                1.0
            },
        })
    }

    /// The width the residual is judged against: the median width of the
    /// attribute's buckets, or 2% of the drawn values' range without any.
    pub fn bucket_width(&self) -> f64 {
        let mut w: Vec<f64> = self
            .spans
            .iter()
            .map(|s| s.high - s.low)
            .filter(|w| *w > 0.0)
            .collect();
        if w.is_empty() {
            let (lo, hi) = self
                .points
                .iter()
                .fold((f64::INFINITY, f64::NEG_INFINITY), |a, p| {
                    (a.0.min(p.drawn), a.1.max(p.drawn))
                });
            return ((hi - lo) * 0.02).max(1e-9);
        }
        w.sort_by(f64::total_cmp);
        w[w.len() / 2]
    }

    /// What the fit says about the knob ([`Calibration`]).
    pub fn calibration(&self) -> Calibration {
        let n = self.points.len();
        let Some(fit) = self.fit().filter(|_| n >= MIN_FIT_RUNS) else {
            return Calibration::TooFew { runs: n };
        };
        if fit.a.abs() < MIN_SLOPE {
            return Calibration::Flat { a: fit.a };
        }
        let width = self.bucket_width();
        // A residual that follows another drawn attribute.
        let residuals: Vec<f64> = self
            .points
            .iter()
            .map(|p| p.measured - (fit.a * p.drawn + fit.b))
            .collect();
        let mut names: Vec<&String> = Vec::new();
        for p in &self.points {
            for (k, _) in &p.others {
                if !names.contains(&k) {
                    names.push(k);
                }
            }
        }
        let worst = names
            .iter()
            .filter_map(|name| {
                let (ys, xs): (Vec<f64>, Vec<&Json>) = self
                    .points
                    .iter()
                    .zip(&residuals)
                    .filter_map(|(p, r)| {
                        p.others
                            .iter()
                            .find(|(k, _)| k == *name)
                            .map(|(_, v)| (*r, v))
                    })
                    .unzip();
                explained(&ys, &xs).map(|s| ((*name).clone(), s))
            })
            .max_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((attribute, share)) = worst {
            if share > MAX_STRUCTURE && fit.residual_std > 0.1 * width {
                return Calibration::Structured { attribute, share };
            }
        }
        if fit.residual_std > MAX_RESIDUAL * width {
            return Calibration::Noisy {
                residual_std: fit.residual_std,
                width,
            };
        }
        // Calibrated when the fit stays within its noise of measured =
        // drawn over the drawn range.
        let (lo, hi) = self
            .points
            .iter()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |a, p| {
                (a.0.min(p.drawn), a.1.max(p.drawn))
            });
        let off = |d: f64| ((fit.a - 1.0) * d + fit.b).abs();
        if off(lo).max(off(hi)) <= (2.0 * fit.residual_std).max(NEGLIGIBLE * width) {
            return Calibration::Calibrated;
        }
        Calibration::Suggest { a: fit.a, b: fit.b }
    }
}

/// A categorical attribute's drift: `(drawn, measured, runs)`.
#[derive(Debug, Clone, PartialEq)]
pub struct CategoricalDrift {
    pub name: String,
    /// The labels in order (the space's, then any other).
    pub labels: Vec<String>,
    pub counts: Vec<(String, String, usize)>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AttributeDrift {
    Numeric(NumericDrift),
    Categorical(CategoricalDrift),
}

/// What one attribute's drift sums up to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Summary {
    pub runs: usize,
    /// Numeric only: mean of measured - drawn, mean |error|, max |error|.
    pub bias: Option<f64>,
    pub mae: Option<f64>,
    pub max_error: Option<f64>,
    /// The share of runs whose measured value is in the drawn one's bucket
    /// (the same label, for a categorical attribute).
    pub bucket_hit: Option<f64>,
    /// The share of runs whose measured value differs from the drawn one by
    /// more than a hundredth of the attribute's range (numeric), or at all.
    pub shifted: Option<f64>,
}

impl AttributeDrift {
    pub fn name(&self) -> &str {
        match self {
            AttributeDrift::Numeric(n) => &n.name,
            AttributeDrift::Categorical(c) => &c.name,
        }
    }

    pub fn summary(&self) -> Summary {
        match self {
            AttributeDrift::Numeric(n) => {
                let k = n.points.len();
                if k == 0 {
                    return Summary {
                        runs: 0,
                        bias: None,
                        mae: None,
                        max_error: None,
                        bucket_hit: None,
                        shifted: None,
                    };
                }
                let errs: Vec<f64> = n.points.iter().map(DriftPoint::error).collect();
                let kf = k as f64;
                let (lo, hi) = n
                    .points
                    .iter()
                    .fold((f64::INFINITY, f64::NEG_INFINITY), |a, p| {
                        (a.0.min(p.drawn), a.1.max(p.drawn))
                    });
                let tol = ((hi - lo).abs() / 100.0).max(1e-9);
                Summary {
                    runs: k,
                    bias: Some(errs.iter().sum::<f64>() / kf),
                    mae: Some(errs.iter().map(|e| e.abs()).sum::<f64>() / kf),
                    max_error: Some(errs.iter().map(|e| e.abs()).fold(0.0, f64::max)),
                    bucket_hit: (!n.spans.is_empty())
                        .then(|| n.points.iter().filter(|p| p.same_bucket()).count() as f64 / kf),
                    shifted: Some(errs.iter().filter(|e| e.abs() > tol).count() as f64 / kf),
                }
            }
            AttributeDrift::Categorical(c) => {
                let k: usize = c.counts.iter().map(|x| x.2).sum();
                let same: usize = c.counts.iter().filter(|x| x.0 == x.1).map(|x| x.2).sum();
                Summary {
                    runs: k,
                    bias: None,
                    mae: None,
                    max_error: None,
                    bucket_hit: (k > 0).then(|| same as f64 / k as f64),
                    shifted: (k > 0).then(|| (k - same) as f64 / k as f64),
                }
            }
        }
    }

    pub fn to_json(&self) -> Json {
        let s = self.summary();
        let summary = json!({
            "runs": s.runs, "bias": s.bias, "mae": s.mae, "max_error": s.max_error,
            "bucket_hit_rate": s.bucket_hit, "shifted": s.shifted,
        });
        match self {
            AttributeDrift::Numeric(n) => json!({
                "attribute": n.name, "kind": "numeric", "unit": n.unit,
                "summary": summary,
                "fit": n.fit().map(|f| json!({"runs": f.runs, "a": f.a, "b": f.b,
                                              "residual_std": f.residual_std, "r2": f.r2})),
                "calibration": calibration_json(&n.calibration()),
                "buckets": n.spans.iter().map(|s| json!({"label": s.label, "low": s.low, "high": s.high})).collect::<Vec<_>>(),
                "runs": n.points.iter().map(|p| json!({
                    "case": p.case_number, "document": p.document, "passed": p.passed,
                    "drawn": p.drawn, "measured": p.measured, "error": p.error(),
                    "drawn_bucket": p.drawn_bucket, "measured_bucket": p.measured_bucket,
                })).collect::<Vec<_>>(),
            }),
            AttributeDrift::Categorical(c) => json!({
                "attribute": c.name, "kind": "categorical", "summary": summary,
                "labels": c.labels,
                "counts": c.counts.iter().map(|(d, m, n)| json!({"drawn": d, "measured": m, "runs": n})).collect::<Vec<_>>(),
            }),
        }
    }
}

fn calibration_json(c: &Calibration) -> Json {
    let mut out = json!({"status": c.status()});
    match c {
        Calibration::Suggest { a, b } => {
            // d is written as (1/a) d - b/a through the existing knob.
            out["write_as"] = json!({"scale": 1.0 / a, "offset": -b / a});
        }
        Calibration::TooFew { runs } => out["runs"] = json!(runs),
        Calibration::Flat { a } => out["a"] = json!(a),
        Calibration::Noisy {
            residual_std,
            width,
        } => {
            out["residual_std"] = json!(residual_std);
            out["bucket_width"] = json!(width);
        }
        Calibration::Structured { attribute, share } => {
            out["depends_on"] = json!(attribute);
            out["share"] = json!(share);
        }
        Calibration::Calibrated => {}
    }
    out
}

/// Every attribute's drift, worst first.
#[derive(Debug, Clone, PartialEq)]
pub struct Drift {
    pub attributes: Vec<AttributeDrift>,
    /// Runs selected, and those with no sample to measure by.
    pub runs: usize,
    pub without_samples: usize,
    pub window: f64,
}

/// Worst first: the lowest bucket hit rate, then the largest error relative
/// to the attribute's range (or share of changed labels).
fn badness(a: &AttributeDrift, set: &CaseSet) -> (f64, f64) {
    let s = a.summary();
    let hit = s.bucket_hit.unwrap_or(1.0);
    let size = match a {
        AttributeDrift::Numeric(n) => {
            let span = set
                .axes
                .get(&n.name)
                .map(|ax| {
                    let (lo, hi) = ax.domain();
                    hi - lo
                })
                .unwrap_or(1.0)
                .max(1e-9);
            s.mae.unwrap_or(0.0) / span
        }
        AttributeDrift::Categorical(_) => s.shifted.unwrap_or(0.0),
    };
    (hit, -size)
}

impl Drift {
    pub fn to_json(&self) -> Json {
        self.to_json_with(&IndexMap::new())
    }

    /// [`Drift::to_json`], with each suggested calibration's corrected knob
    /// (`calibration.knob`: the current and the corrected `scale` and
    /// `offset`) for the attributes `knobs` gives the current knob of.
    pub fn to_json_with(&self, knobs: &IndexMap<String, (f64, f64)>) -> Json {
        let attributes: Vec<Json> = self
            .attributes
            .iter()
            .map(|a| {
                let mut j = a.to_json();
                if let (AttributeDrift::Numeric(n), Some(&(scale, offset))) =
                    (a, knobs.get(a.name()))
                {
                    if let Calibration::Suggest { a, b } = n.calibration() {
                        let (s2, o2) = corrected_knob(scale, offset, a, b);
                        j["calibration"]["knob"] = json!({
                            "scale": scale, "offset": offset,
                            "corrected": {"scale": s2, "offset": o2},
                        });
                    }
                }
                j
            })
            .collect();
        json!({
            "window": self.window,
            "runs": self.runs,
            "runs_without_samples": self.without_samples,
            "attributes": attributes,
        })
    }
}

impl CaseSet {
    /// Each attribute's drawn values against the measured ones, over the
    /// runs `query` selects (its scenario, environment and window).
    pub fn drift(&self, query: &CaseQuery) -> Drift {
        let window = query.window();
        let cases = self.selected(&CaseQuery {
            basis: crate::cases::Basis::Drawn,
            ..query.clone()
        });
        let mut runs = 0;
        let mut without = 0;
        let mut out: Vec<AttributeDrift> = Vec::new();
        let slot = |name: &str, out: &mut Vec<AttributeDrift>| -> usize {
            if let Some(i) = out.iter().position(|a| a.name() == name) {
                return i;
            }
            let axis = &self.axes[name];
            out.push(match axis.kind {
                AxisKind::Numeric => AttributeDrift::Numeric(NumericDrift {
                    name: name.to_string(),
                    unit: axis.unit.clone(),
                    spans: axis.spans.clone(),
                    points: Vec::new(),
                }),
                AxisKind::Categorical => AttributeDrift::Categorical(CategoricalDrift {
                    name: name.to_string(),
                    labels: axis.labels.clone(),
                    counts: Vec::new(),
                }),
            });
            out.len() - 1
        };
        for (n, case) in cases.iter().enumerate() {
            for run in &case.runs {
                runs += 1;
                let measured = run.measured(window);
                if measured.is_empty() {
                    without += 1;
                    continue;
                }
                for (name, drawn) in &case.values {
                    let (Some(m), true) = (measured.get(name), self.axes.contains_key(name)) else {
                        continue;
                    };
                    let i = slot(name, &mut out);
                    match &mut out[i] {
                        AttributeDrift::Numeric(a) => {
                            if let (Some(d), Some(mv)) = (drawn.as_f64(), m.as_f64()) {
                                a.points.push(DriftPoint {
                                    case: case.key.clone(),
                                    case_number: n + 1,
                                    document: run.document,
                                    passed: run.passed,
                                    drawn: d,
                                    measured: mv,
                                    drawn_bucket: self.bucket_of(name, drawn),
                                    measured_bucket: self.bucket_of(name, m),
                                    others: case
                                        .values
                                        .iter()
                                        .filter(|(k, _)| *k != name)
                                        .map(|(k, v)| (k.clone(), v.clone()))
                                        .collect(),
                                });
                            }
                        }
                        AttributeDrift::Categorical(a) => {
                            let (d, mv) = (value_text(drawn), value_text(m));
                            for l in [&d, &mv] {
                                if !a.labels.contains(l) {
                                    a.labels.push(l.clone());
                                }
                            }
                            match a.counts.iter_mut().find(|x| x.0 == d && x.1 == mv) {
                                Some(x) => x.2 += 1,
                                None => a.counts.push((d, mv, 1)),
                            }
                        }
                    }
                }
            }
        }
        out.retain(|a| a.summary().runs > 0);
        out.sort_by(|a, b| {
            let (x, y) = (badness(a, self), badness(b, self));
            x.0.total_cmp(&y.0).then(x.1.total_cmp(&y.1))
        });
        Drift {
            attributes: out,
            runs,
            without_samples: without,
            window,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(drawn: f64, measured: f64, other: Json) -> DriftPoint {
        DriftPoint {
            case: String::new(),
            case_number: 1,
            document: 0,
            passed: true,
            drawn,
            measured,
            drawn_bucket: String::new(),
            measured_bucket: String::new(),
            others: vec![("rain".into(), other)],
        }
    }

    fn drift(points: Vec<DriftPoint>) -> NumericDrift {
        NumericDrift {
            name: "gap".into(),
            unit: "m".into(),
            spans: vec![
                Span {
                    label: "[0, 10)".into(),
                    low: 0.0,
                    high: 10.0,
                },
                Span {
                    label: "[10, 20)".into(),
                    low: 10.0,
                    high: 20.0,
                },
            ],
            points,
        }
    }

    #[test]
    fn an_offset_is_found_and_corrected() {
        let d = drift(
            (0..10)
                .map(|i| point(i as f64 * 2.0, i as f64 * 2.0 + 2.2, json!("none")))
                .collect(),
        );
        let f = d.fit().unwrap();
        assert!((f.a - 1.0).abs() < 1e-9 && (f.b - 2.2).abs() < 1e-9 && f.r2 > 0.999);
        assert_eq!(d.calibration(), Calibration::Suggest { a: f.a, b: f.b });
        // The knob composed with the fit's inverse stages the drawn value.
        let (s, o) = corrected_knob(1.0, 11.0, f.a, f.b);
        assert!((s - 1.0).abs() < 1e-9 && (o - 8.8).abs() < 1e-9);
        let all = Drift {
            attributes: vec![AttributeDrift::Numeric(d.clone())],
            runs: 10,
            without_samples: 0,
            window: 1.0,
        };
        let knobs: IndexMap<String, (f64, f64)> = [("gap".to_string(), (1.0, 11.0))].into();
        let j = all.to_json_with(&knobs);
        let k = &j["attributes"][0]["calibration"]["knob"]["corrected"];
        assert!((k["offset"].as_f64().unwrap() - 8.8).abs() < 1e-9);
        assert!(all.to_json()["attributes"][0]["calibration"]["knob"].is_null());
        let (s, o) = corrected_knob(2.0, 1.0, 0.5, 3.0);
        // written = s/a d + o - s b/a; staged = a * (written-knob value) + b
        assert!((s - 4.0).abs() < 1e-9 && (o - (1.0 - 12.0)).abs() < 1e-9);
    }

    #[test]
    fn untrusted_fits_say_why() {
        assert!(matches!(
            drift(
                (0..3)
                    .map(|i| point(i as f64, i as f64 + 1.0, json!("none")))
                    .collect()
            )
            .calibration(),
            Calibration::TooFew { runs: 3 }
        ));
        let flat = drift(
            (0..10)
                .map(|i| point(i as f64 * 2.0, 5.0 + (i % 2) as f64 * 0.01, json!("none")))
                .collect(),
        );
        assert!(matches!(flat.calibration(), Calibration::Flat { .. }));
        // A residual that follows the rain: no offset or scale fixes it.
        let structured = drift(
            (0..12)
                .map(|i| {
                    let wet = i % 2 == 0;
                    point(
                        i as f64 * 1.5,
                        i as f64 * 1.5 + if wet { 4.0 } else { 0.0 },
                        json!(if wet { "heavy" } else { "none" }),
                    )
                })
                .collect(),
        );
        assert!(
            matches!(structured.calibration(), Calibration::Structured { ref attribute, .. } if attribute == "rain")
        );
        let noisy = drift(
            (0..12)
                .map(|i| {
                    point(
                        i as f64 * 1.5,
                        i as f64 * 1.5 + [3.0, -3.0, 2.0, -2.5][i % 4],
                        json!("none"),
                    )
                })
                .collect(),
        );
        assert!(matches!(noisy.calibration(), Calibration::Noisy { .. }));
        let fine = drift(
            (0..10)
                .map(|i| point(i as f64 * 2.0, i as f64 * 2.0 + 0.1, json!("none")))
                .collect(),
        );
        assert_eq!(fine.calibration(), Calibration::Calibrated);
    }
}
