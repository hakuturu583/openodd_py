//! Fitting rules to data: maximum-likelihood estimates of the parts'
//! parameters from values measured together -- a log of the rain and the
//! grip a vehicle found in it, say.
//!
//! The rules are a factor graph: each quantity is a conditional
//! distribution given the variables its parts read ([`crate::rules`]). Given
//! rows of observed values, the likelihood of the data is the product, over
//! the rows and the quantities, of each quantity's density at its observed
//! value given the values its parts read there. It factors by quantity, so
//! each quantity is fitted on its own, from the rows that observe it and
//! everything it reads (a row missing one of them says nothing of that
//! factor, and is skipped for it; an input missing from a row reads the
//! `default` the rules give it).
//!
//! What is estimated is the rule file's own numbers: every parameter at
//! every point a part writes, each value a row reads being interpolated
//! from them ([`Part::weights`]). So the likelihood is a function of those
//! numbers, maximized by L-BFGS from the rules' current values, under the
//! constraint that every distribution stays valid at every point (a
//! triangular's `mode` is fitted as its share of the range, a normal's `sd`
//! on a log scale) and every row lies within the range of a uniform or a
//! triangular (a log barrier, its weight lowered to 0 stage by stage). A point
//! no row falls near keeps its value. Family by family:
//!
//! - `uniform`: `low` and `high`.
//! - `triangular`: `low`, `mode` and `high`.
//! - `normal`: `mean` and `sd`. Its truncation `low`..`high` is the
//!   physical range, held as written; a row outside it is an error.
//!
//! With several parts, the parts move together (one quantile), so the
//! quantity's distribution function is the greatest of the parts' (`min`)
//! or the least (`max`) and its density at a value is the density of the
//! part deciding it there ([`crate::rules::Joined::cdf`]).
//!
//! The width of a distribution at a point with data -- `high - low`, or a
//! normal's spread as truncated ([`Family::spread`]) -- is kept at least
//! `min_spread` times the spread of the quantity's observed values: a
//! likelihood grows without bound as a distribution narrows onto one value
//! (or, for a normal, as its mean runs off its range and squeezes it against
//! an end), which is no estimate.

use std::cell::RefCell;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::rules::{Combine, Dist, Family, Given, Hold, Origin, Quantity, RuleSet, Truncated};
use crate::value::py_float_repr;

/// How [`RuleSet::fit`] fits.
#[derive(Debug, Clone, PartialEq)]
pub struct FitOptions {
    /// The quantities to fit; `None`: every quantity some row observes.
    pub quantities: Option<Vec<String>>,
    /// The most L-BFGS iterations of each stage of a quantity's fit (one
    /// stage, or one per weight of the barrier on a uniform's or a
    /// triangular's range).
    pub max_iterations: usize,
    /// Converged when an iteration raises the mean log-likelihood per row by
    /// less than this.
    pub tolerance: f64,
    /// The least width of a distribution at a point with data, as a share
    /// of the spread (standard deviation) of the quantity's observed values.
    pub min_spread: f64,
    /// Values held as written, besides those the rules' own `fixed` holds:
    /// `quantity`, `quantity.part`, `quantity.part.param`,
    /// `quantity.part.param@input=point,...` or `quantity.part@input=point,...`
    /// ([`crate::rules::Hold`]).
    pub fixed: Vec<String>,
    /// The data files the rows were read from, `(path or name, sha256)`:
    /// what each fitted part's `basis:` names as its data.
    pub data_files: Vec<(String, String)>,
    /// Who fitted, for `basis.by` (by default this library and its version).
    pub by: String,
    /// When, for `basis.date`, as the caller writes it; empty: not written.
    pub date: String,
}

impl Default for FitOptions {
    fn default() -> Self {
        FitOptions {
            data_files: Vec::new(),
            by: format!("openodd {}", env!("CARGO_PKG_VERSION")),
            date: String::new(),
            quantities: None,
            max_iterations: 1000,
            tolerance: 1e-10,
            min_spread: 1e-3,
            fixed: Vec::new(),
        }
    }
}

/// What one part's fit touched.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PartFit {
    pub name: String,
    /// The points it writes.
    pub points: usize,
    /// The points some row reads (interpolates from).
    pub points_with_data: usize,
    /// The values estimated: each free parameter at each point with data
    /// (a value held, by the rules' `fixed` or the fit's, is not).
    pub estimated: usize,
}

/// One quantity's fit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuantityFit {
    pub name: String,
    /// The rows that observe it and everything it reads.
    pub rows: usize,
    /// The rows that observe it but miss something it reads.
    pub skipped: usize,
    /// The log-likelihood of those rows (weighted) under the rules as they
    /// were: `-inf` when a row lies outside a distribution's range (`null`
    /// in JSON).
    #[serde(with = "or_minus_infinity")]
    pub log_likelihood_before: f64,
    /// ... and under the fitted rules.
    #[serde(with = "or_minus_infinity")]
    pub log_likelihood: f64,
    pub iterations: usize,
    /// Whether the fit stopped by converging, not at `max_iterations`.
    pub converged: bool,
    pub parts: Vec<PartFit>,
    /// How well the rules as they were match the rows ...
    pub calibration_before: Calibration,
    /// ... and how well the fitted rules do.
    pub calibration: Calibration,
}

/// How well a quantity's distribution matches rows of data, from where
/// each row's value falls in the distribution at that row's inputs: its
/// quantile `u = cdf(value)` (the probability integral transform). Under
/// rules that match the data, `u` is spread evenly over 0..1 -- whatever
/// the distribution's family. Shares are of the rows' weight.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Calibration {
    /// The rows (giving the quantity and all it reads).
    pub rows: usize,
    /// The log-likelihood per unit of weight: comparable across data sets
    /// of different sizes (`-inf` when a row is impossible).
    #[serde(with = "or_minus_infinity")]
    pub mean_log_likelihood: f64,
    /// The share of rows within the distribution's central 90% (`u` from
    /// 0.05 to 0.95): 0.9 when it matches, less when the rules are too
    /// narrow or off-centre, more when too wide.
    pub in_band: f64,
    /// The share of rows the rules hold impossible (outside its range).
    pub outside: f64,
    /// Kolmogorov-Smirnov's distance of `u` from even: the greatest gap
    /// between the share of rows at or below a quantile and the quantile
    /// (0: even; about `1.36 / sqrt(rows)` is what chance alone gives 5% of
    /// the time).
    pub ks: f64,
    /// The share of rows in each tenth of the quantiles (0-0.1, ...,
    /// 0.9-1): flat when it matches; a U when too narrow, a hump when too
    /// wide, a slope when off-centre.
    pub histogram: Vec<f64>,
}

/// A log-likelihood in JSON: a number, or `null` for `-inf`, which JSON
/// does not write.
mod or_minus_infinity {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &f64, s: S) -> Result<S::Ok, S::Error> {
        if v.is_finite() {
            s.serialize_f64(*v)
        } else {
            s.serialize_none()
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
        Ok(Option::<f64>::deserialize(d)?.unwrap_or(f64::NEG_INFINITY))
    }
}

/// How far a fit has come ([`RuleSet::fit_with_progress`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Progress {
    /// The share of the work done, 0 to 1: an estimate, each quantity
    /// weighed by its rows, parts and stages, and within one the stages
    /// done and how far the current one has come.
    pub fraction: f64,
    /// The quantity being fitted.
    pub quantity: String,
    /// Its place among the quantities fitted (from 0), and how many there are.
    pub index: usize,
    pub count: usize,
}

/// A fit: the fitted rules, how each quantity was fitted, and the fitted
/// parts as a rule file ([`Fit::text`]).
#[derive(Debug, Clone)]
pub struct Fit {
    /// The rules, every fitted part's distribution replaced.
    pub rules: RuleSet,
    pub quantities: Vec<QuantityFit>,
    /// The fitted parts: a rule file's `quantities:` section.
    body: String,
}

impl Fit {
    /// The fitted parts as a rule file that merges over the rules fitted;
    /// with `extends`, one extending that reference (the rules fitted), so
    /// it reads as the whole fitted set.
    pub fn text(&self, extends: Option<&str>) -> String {
        rule_file(extends, &self.body)
    }
}

/// A rule file: `version: 2`, `extends` if given, and `body`.
fn rule_file(extends: Option<&str>, body: &str) -> String {
    let mut out = String::from("version: 2\n");
    if let Some(e) = extends {
        let quoted = serde_json::to_string(e).expect("a string serializes");
        out.push_str(&format!("extends: {quoted}\n"));
    }
    out.push_str(body);
    out
}

/// A value of a row of data as it was read, before it is a number.
#[derive(Debug, Clone, PartialEq)]
pub enum Cell {
    /// Not given: `null`, `None`, an empty string.
    Missing,
    Number(f64),
    Text(String),
    /// Anything else -- a list, a truth value -- as written.
    Other(String),
}

impl Cell {
    /// The number it is: `None` when not observed (missing, NaN), an error
    /// when not a number. Text reads as Python's `float()` reads it.
    fn number(&self, row: usize, key: &str) -> Result<Option<f64>> {
        let x = match self {
            Cell::Missing => return Ok(None),
            Cell::Number(x) => Some(*x),
            Cell::Text(s) if s.trim().is_empty() => return Ok(None),
            Cell::Text(s) => crate::value::parse_float(s.trim()),
            Cell::Other(_) => None,
        };
        match x {
            Some(x) if x.is_nan() => Ok(None),
            Some(x) if x.is_finite() => Ok(Some(x)),
            _ => Err(Error::value(format!(
                "row {row}: {key} is {}, not a number",
                match self {
                    Cell::Text(s) => format!("{s:?}"),
                    Cell::Number(x) => py_float_repr(*x),
                    Cell::Other(s) => s.clone(),
                    Cell::Missing => "missing".into(),
                }
            ))),
        }
    }
}

/// A row of data as read: name -> value.
pub type Row = IndexMap<String, Cell>;

/// Rows of data, as a fit reads them ([`observations`]).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Observations {
    /// Per row, the values it gives of the names asked for.
    pub rows: Vec<IndexMap<String, f64>>,
    /// The weight column, and per row its weight (1 where a row has none),
    /// when the data names one.
    pub weight: Option<(String, Vec<f64>)>,
    /// Per name asked for, the rows that give it.
    pub observed: IndexMap<String, usize>,
}

impl Observations {
    /// Per row, its weight, when the data names a weight column.
    pub fn weights(&self) -> Option<&[f64]> {
        self.weight.as_ref().map(|(_, w)| w.as_slice())
    }
}

/// `rows` as a fit reads them: the values of `names` (the quantities and
/// inputs the rules read; every other key -- a timestamp, an id -- is
/// ignored), each a number or not observed, and each row's weight from the
/// column `weight` names (1 where a row has none; a negative one is an
/// error).
pub fn observations(rows: &[Row], names: &[String], weight: Option<&str>) -> Result<Observations> {
    observations_labelled(rows, names, weight, &IndexMap::new())
}

/// [`observations`], a categorical input's value read as its label's index
/// in `labels` (input -> its labels: [`RuleSet::input_labels`]).
pub fn observations_labelled(
    rows: &[Row],
    names: &[String],
    weight: Option<&str>,
    labels: &IndexMap<String, Vec<String>>,
) -> Result<Observations> {
    let mut out = Observations {
        observed: names.iter().map(|n| (n.clone(), 0)).collect(),
        ..Observations::default()
    };
    let mut weights = Vec::new();
    for (r, row) in rows.iter().enumerate() {
        let mut values = IndexMap::new();
        for name in names {
            let read = |c: &Cell| -> Result<Option<f64>> {
                let Some(l) = labels.get(name) else {
                    return c.number(r, name);
                };
                match c {
                    // Not observed, as for a number: empty, or NaN.
                    Cell::Missing => Ok(None),
                    Cell::Text(t) if t.trim().is_empty() => Ok(None),
                    Cell::Number(x) if x.is_nan() => Ok(None),
                    Cell::Text(t) | Cell::Other(t) => l
                        .iter()
                        .position(|x| x == t.trim())
                        .map(|i| Some(i as f64))
                        .ok_or_else(|| {
                            Error::value(format!(
                                "row {r}: {name} is {t:?}, not one of its labels {}",
                                l.join(", ")
                            ))
                        }),
                    Cell::Number(x) => Err(Error::value(format!(
                        "row {r}: {name} is {}, not one of its labels {}",
                        py_float_repr(*x),
                        l.join(", ")
                    ))),
                }
            };
            if let Some(x) = row.get(name).map(read).transpose()?.flatten() {
                values.insert(name.clone(), x);
                out.observed[name] += 1;
            }
        }
        if let Some(w) = weight {
            match row.get(w).map(|c| c.number(r, w)).transpose()?.flatten() {
                Some(x) if x < 0.0 => {
                    return Err(Error::value(format!("row {r}: its weight {w} is negative")))
                }
                Some(x) => weights.push(x),
                None => weights.push(1.0),
            }
        }
        out.rows.push(values);
    }
    out.weight = weight.map(|w| (w.to_string(), weights));
    Ok(out)
}

/// The rows of a JSON data file, and the weight column it names. The file
/// is one of:
///
/// - `{"rows": [{name: value, ...}, ...], "weight": "<column>"}` (`weight`
///   optional);
/// - a list of rows, `[{name: value, ...}, ...]`;
/// - columns, `{name: [value, ...], ...}`, each of one length.
///
/// A row need not give every name: one it leaves out, or gives as `null`,
/// is not observed there.
pub fn json_rows(text: &str) -> Result<(Vec<Row>, Option<String>)> {
    use serde_json::Value as J;
    let doc: J = serde_json::from_str(text).map_err(|e| Error::value(format!("not JSON: {e}")))?;
    let cell = |v: &J| match v {
        J::Null => Cell::Missing,
        J::Number(n) => n
            .as_f64()
            .map_or_else(|| Cell::Other(v.to_string()), Cell::Number),
        J::String(s) => Cell::Text(s.clone()),
        other => Cell::Other(other.to_string()),
    };
    let rows_of = |items: Vec<J>| -> Result<Vec<Row>> {
        items
            .into_iter()
            .enumerate()
            .map(|(i, item)| match item {
                J::Object(m) => Ok(m.iter().map(|(k, v)| (k.clone(), cell(v))).collect()),
                other => Err(Error::value(format!(
                    "row {i} is {other}: a row is an object, name: value"
                ))),
            })
            .collect()
    };
    match doc {
        J::Array(items) => Ok((rows_of(items)?, None)),
        J::Object(mut map) if map.contains_key("rows") => {
            let weight = match map.remove("weight") {
                None | Some(J::Null) => None,
                Some(J::String(w)) => Some(w),
                Some(_) => return Err(Error::value("\"weight\" names a column: a string")),
            };
            if let Some(extra) = map.keys().find(|k| *k != "rows") {
                return Err(Error::value(format!(
                    "unknown key \"{extra}\": the data is {{\"rows\": [...], \"weight\": \"<column>\"}}"
                )));
            }
            match map.remove("rows") {
                Some(J::Array(items)) => Ok((rows_of(items)?, weight)),
                _ => Err(Error::value("\"rows\" is a list of rows")),
            }
        }
        J::Object(columns) => {
            let mut n = None;
            for (k, v) in &columns {
                let J::Array(values) = v else {
                    return Err(Error::value(format!(
                        "\"{k}\" is not a column (a list): the data is a list of rows, \
                         {{\"rows\": [...]}}, or columns"
                    )));
                };
                match n {
                    None => n = Some(values.len()),
                    Some(m) if m != values.len() => {
                        return Err(Error::value(format!(
                            "the columns are not all of one length: \"{k}\" has {}, not {m}",
                            values.len()
                        )))
                    }
                    _ => {}
                }
            }
            let rows = (0..n.unwrap_or(0))
                .map(|i| {
                    columns
                        .iter()
                        .map(|(k, v)| (k.clone(), cell(&v[i])))
                        .collect()
                })
                .collect();
            Ok((rows, None))
        }
        _ => Err(Error::value(
            "the data is a list of rows, {\"rows\": [...]}, or columns",
        )),
    }
}

/// [`json_rows`] read as a fit of `set` reads them ([`observations_labelled`]):
/// the values `set` reads, a categorical input's as its label's index.
pub fn observations_from_json(text: &str, set: &RuleSet) -> Result<Observations> {
    let (rows, weight) = json_rows(text)?;
    observations_labelled(
        &rows,
        &set.data_names(),
        weight.as_deref(),
        &set.input_labels(),
    )
}

/// A row's value of `name`, if it has a finite one.
fn observed(row: &IndexMap<String, f64>, name: &str) -> Option<f64> {
    row.get(name).copied().filter(|v| v.is_finite())
}

/// One quantity's data: per row, its value, its weight, and per part the
/// points the part's parameters there are interpolated from.
struct Data {
    y: Vec<f64>,
    weight: Vec<f64>,
    /// `[row][part]`: (point, weight).
    corners: Vec<Vec<Vec<(usize, f64)>>>,
    skipped: usize,
}

/// A quantity ready to fit: its data, what is estimated, and how many
/// stages its fit takes.
struct Prepared<'a> {
    q: &'a Quantity,
    data: Data,
    /// Per part, the points some row reads, in order.
    touched: Vec<Vec<usize>>,
    /// `[part][param][point]`: whether the value is estimated.
    free: Vec<Vec<Vec<bool>>>,
}

impl Prepared<'_> {
    fn new<'a>(q: &'a Quantity, data: Data, held: &[Vec<Hold>]) -> Prepared<'a> {
        let touched = q
            .parts
            .iter()
            .enumerate()
            .map(|(i, part)| {
                let mut seen = vec![false; part.size()];
                for corners in &data.corners {
                    for &(n, _) in &corners[i] {
                        seen[n] = true;
                    }
                }
                (0..seen.len()).filter(|&n| seen[n]).collect()
            })
            .collect();
        let free = q
            .parts
            .iter()
            .zip(held)
            .map(|(part, holds)| {
                (0..part.family.params().len())
                    .map(|k| {
                        (0..part.size())
                            .map(|n| {
                                part.family.fitted_params().contains(&k)
                                    && !holds.iter().any(|h| h.holds(k, n))
                            })
                            .collect()
                    })
                    .collect()
            })
            .collect();
        Prepared {
            q,
            data,
            touched,
            free,
        }
    }

    /// The estimated values, as (part, parameter, point).
    fn vars(&self) -> Vec<(usize, usize, usize)> {
        let mut vars = Vec::new();
        for (i, free) in self.free.iter().enumerate() {
            for (k, at) in free.iter().enumerate() {
                vars.extend(
                    self.touched[i]
                        .iter()
                        .filter(|&&n| at[n])
                        .map(|&n| (i, k, n)),
                );
            }
        }
        vars
    }

    /// The barrier's stages when the fit moves a uniform's or a
    /// triangular's range, else the likelihood alone.
    fn stages(&self) -> &'static [f64] {
        let moves_range = self.q.parts.iter().enumerate().any(|(i, part)| {
            part.family.fitted_range().is_some_and(|(lo, hi)| {
                self.touched[i]
                    .iter()
                    .any(|&n| self.free[i][lo][n] || self.free[i][hi][n])
            })
        });
        if moves_range {
            &BARRIER
        } else {
            &[0.0]
        }
    }
}

impl RuleSet {
    /// The log-likelihood of `rows` (each observed values by name; a value
    /// missing or not finite is not observed), per quantity some row
    /// observes along with everything it reads: `(log-likelihood, rows)`.
    /// `weights` weighs each row (1 when not given).
    pub fn log_likelihood(
        &self,
        rows: &[IndexMap<String, f64>],
        weights: Option<&[f64]>,
    ) -> Result<IndexMap<String, (f64, usize)>> {
        check_weights(rows, weights)?;
        let mut out = IndexMap::new();
        for name in self.order() {
            let q = self.quantity(name)?;
            let data = self.data_of(q, rows, weights);
            if data.y.is_empty() {
                continue;
            }
            out.insert(
                name.clone(),
                (log_likelihood_of(q, &start_params(q), &data), data.y.len()),
            );
        }
        Ok(out)
    }

    /// Per quantity some row gives with everything it reads, how well these
    /// rules match `rows` ([`Calibration`]); `weights` weighs each row.
    pub fn calibration(
        &self,
        rows: &[IndexMap<String, f64>],
        weights: Option<&[f64]>,
    ) -> Result<IndexMap<String, Calibration>> {
        check_weights(rows, weights)?;
        let mut out = IndexMap::new();
        for name in self.order() {
            let q = self.quantity(name)?;
            let data = self.data_of(q, rows, weights);
            if !data.y.is_empty() {
                out.insert(name.clone(), calibration_of(q, &start_params(q), &data));
            }
        }
        Ok(out)
    }

    /// Per quantity, the rows a fit would read -- those giving it and every
    /// value it reads -- and those it would skip (giving it, but missing
    /// something it reads).
    pub fn usable(&self, rows: &[IndexMap<String, f64>]) -> IndexMap<String, (usize, usize)> {
        self.order()
            .iter()
            .map(|name| {
                let q = &self.quantities[name];
                let inputs = q.inputs();
                let (mut read, mut skipped) = (0, 0);
                for row in rows.iter().filter(|row| observed(row, name).is_some()) {
                    let has = |i: &String| {
                        observed(row, i).is_some()
                            || self.inputs.get(i).and_then(|i| i.default).is_some()
                    };
                    if inputs.iter().all(has) {
                        read += 1;
                    } else {
                        skipped += 1;
                    }
                }
                (name.clone(), (read, skipped))
            })
            .collect()
    }

    /// The names a row of data may give these rules: the quantities, and
    /// the inputs their parts read.
    /// Each categorical input's labels: what a row gives it as.
    pub fn input_labels(&self) -> IndexMap<String, Vec<String>> {
        self.inputs
            .iter()
            .filter(|(_, i)| !i.labels.is_empty())
            .map(|(n, i)| (n.clone(), i.labels.clone()))
            .collect()
    }

    pub fn data_names(&self) -> Vec<String> {
        self.order()
            .iter()
            .cloned()
            .chain(self.free_inputs())
            .collect()
    }

    /// These rules with a rule file's text merged over them (a fit's
    /// [`Fit::text`], say).
    pub fn with_overlay(&self, text: &str) -> Result<RuleSet> {
        self.parse_over(
            text,
            "<fitted rules>",
            &Origin::Text,
            &mut Given(IndexMap::new()),
        )
    }

    /// The maximum-likelihood fit of these rules' parameters to `rows`
    /// (see the module's documentation).
    pub fn fit(
        &self,
        rows: &[IndexMap<String, f64>],
        weights: Option<&[f64]>,
        options: &FitOptions,
    ) -> Result<Fit> {
        self.fit_with_progress(rows, weights, options, &mut |_| {})
    }

    /// [`RuleSet::fit`], telling `progress` how far it has come as it goes:
    /// at the start of each quantity, and at every iteration.
    pub fn fit_with_progress(
        &self,
        rows: &[IndexMap<String, f64>],
        weights: Option<&[f64]>,
        options: &FitOptions,
        progress: &mut dyn FnMut(&Progress),
    ) -> Result<Fit> {
        let (body, quantities) = self.fit_parts(rows, weights, options, progress)?;
        let rules = self
            .with_overlay(&rule_file(None, &body))
            .map_err(|e| Error::value(format!("the fitted rules are not valid: {e}")))?;
        Ok(Fit {
            rules,
            quantities,
            body,
        })
    }

    /// [`RuleSet::fit_with_progress`] without the fitted rules: the fitted
    /// parts as a rule file that merges over these ([`Fit::text`]), and the
    /// report -- what a caller that merges them elsewhere needs.
    pub fn fit_text_with_progress(
        &self,
        rows: &[IndexMap<String, f64>],
        weights: Option<&[f64]>,
        options: &FitOptions,
        progress: &mut dyn FnMut(&Progress),
    ) -> Result<(String, Vec<QuantityFit>)> {
        let (body, quantities) = self.fit_parts(rows, weights, options, progress)?;
        Ok((rule_file(None, &body), quantities))
    }

    /// The fit's work: the fitted parts' `quantities:` section, and the report.
    fn fit_parts(
        &self,
        rows: &[IndexMap<String, f64>],
        weights: Option<&[f64]>,
        options: &FitOptions,
        progress: &mut dyn FnMut(&Progress),
    ) -> Result<(String, Vec<QuantityFit>)> {
        check_weights(rows, weights)?;
        if !(options.min_spread > 0.0) {
            return Err(Error::value(format!(
                "min_spread must be positive, not {}",
                py_float_repr(options.min_spread)
            )));
        }
        if !(options.tolerance >= 0.0) {
            return Err(Error::value(format!(
                "tolerance must not be negative, not {}",
                py_float_repr(options.tolerance)
            )));
        }
        let wanted: Vec<String> = match &options.quantities {
            Some(names) => {
                for n in names {
                    self.quantity(n)?;
                }
                self.order()
                    .iter()
                    .filter(|q| names.contains(q))
                    .cloned()
                    .collect()
            }
            None => self.order().to_vec(),
        };
        // Per quantity, per part: what is held, the rules' own and the fit's.
        let mut holds: IndexMap<String, Vec<Vec<Hold>>> = self
            .quantities
            .iter()
            .map(|(n, q)| (n.clone(), q.parts.iter().map(|p| p.fixed.clone()).collect()))
            .collect();
        for spec in &options.fixed {
            let (quantity, part, hold) = self.fixed_of(spec)?;
            let parts = &mut holds[&quantity];
            match part {
                Some(i) => parts[i].push(hold),
                None => parts.iter_mut().for_each(|p| p.push(Hold::ALL)),
            }
        }
        let mut todo = Vec::new();
        for name in &wanted {
            let q = self.quantity(name)?;
            let held = &holds[name];
            if held.iter().all(|h| h.contains(&Hold::ALL)) {
                continue;
            }
            let data = self.data_of(q, rows, weights);
            if data.y.is_empty() {
                if options.quantities.is_some() {
                    return Err(Error::value(format!(
                        "cannot fit {name}: no row gives it{}",
                        match self.inputs_of(q).as_slice() {
                            [] => String::new(),
                            reads => format!(" and every value it reads ({})", reads.join(", ")),
                        }
                    )));
                }
                continue;
            }
            todo.push(Prepared::new(q, data, held));
        }
        // The work each quantity is, roughly: its rows, times the
        // distributions each row reads, times the stages of its fit.
        let cost: Vec<f64> = todo
            .iter()
            .map(|p| (p.data.y.len() * p.q.parts.len() * p.stages().len()) as f64)
            .collect();
        let total: f64 = cost.iter().sum::<f64>().max(1.0);
        let mut done = 0.0;
        let mut reports = Vec::new();
        let mut fitted: Vec<(&Quantity, Vec<Vec<Vec<f64>>>)> = Vec::new();
        let count = todo.len();
        for (index, prepared) in todo.into_iter().enumerate() {
            let (q, share) = (prepared.q, cost[index]);
            let mut tell = |within: f64| {
                progress(&Progress {
                    fraction: ((done + share * within.clamp(0.0, 1.0)) / total).min(1.0),
                    quantity: q.name.clone(),
                    index,
                    count,
                })
            };
            tell(0.0);
            let (params, report) = fit_quantity(prepared, options, &mut tell)?;
            done += share;
            reports.push(report);
            fitted.push((q, params));
        }
        Ok((quantities_text(&fitted, &reports, options), reports))
    }

    /// What a fit's `fixed` entry names: the quantity, the part (`None`:
    /// every one) and the values of it.
    fn fixed_of(&self, spec: &str) -> Result<(String, Option<usize>, Hold)> {
        let bad = |e: String| Error::value(format!("fixed '{spec}': {e}"));
        let (path, at) = match spec.split_once('@') {
            Some((p, a)) => (p, Some(a)),
            None => (spec, None),
        };
        let names: Vec<&str> = path.trim().splitn(3, '.').collect();
        let q = self.quantity(names[0]).map_err(|e| bad(e.to_string()))?;
        let Some(part_name) = names.get(1) else {
            if at.is_some() {
                return Err(bad(
                    "a point is named in a part: quantity.part@input=point".into()
                ));
            }
            return Ok((q.name.clone(), None, Hold::ALL));
        };
        let Some(i) = q.parts.iter().position(|p| p.name == *part_name) else {
            let known: Vec<&str> = q.parts.iter().map(|p| p.name.as_str()).collect();
            return Err(bad(format!(
                "{} has no part '{part_name}'; it has {}",
                q.name,
                known.join(", ")
            )));
        };
        let part = &q.parts[i];
        let hold = Hold::new(names.get(2).copied(), at, part.family, &part.axes).map_err(bad)?;
        Ok((q.name.clone(), Some(i), hold))
    }

    /// The values `q`'s parts read that have no default: what a row must give.
    fn inputs_of(&self, q: &Quantity) -> Vec<String> {
        q.inputs()
            .into_iter()
            .filter(|i| self.inputs.get(i).and_then(|i| i.default).is_none())
            .collect()
    }

    fn data_of(
        &self,
        q: &Quantity,
        rows: &[IndexMap<String, f64>],
        weights: Option<&[f64]>,
    ) -> Data {
        let mut data = Data {
            y: Vec::new(),
            weight: Vec::new(),
            corners: Vec::new(),
            skipped: 0,
        };
        'rows: for (r, row) in rows.iter().enumerate() {
            let Some(y) = observed(row, &q.name) else {
                continue;
            };
            let w = weights.map_or(1.0, |w| w[r]);
            if w == 0.0 {
                continue;
            }
            let mut corners = Vec::with_capacity(q.parts.len());
            for part in &q.parts {
                let mut values = Vec::with_capacity(part.axes.len());
                for a in &part.axes {
                    let v = observed(row, &a.input)
                        .or_else(|| self.inputs.get(&a.input).and_then(|i| i.default));
                    match v {
                        // A label the part is not written at: the quantity
                        // has no distribution there, so the row says nothing
                        // of it (as one missing an input).
                        Some(v) if a.categorical() && !a.points.contains(&v) => {
                            data.skipped += 1;
                            continue 'rows;
                        }
                        Some(v) => values.push(v),
                        None => {
                            data.skipped += 1;
                            continue 'rows;
                        }
                    }
                }
                corners.push(part.weights(&values));
            }
            data.y.push(y);
            data.weight.push(w);
            data.corners.push(corners);
        }
        data
    }
}

fn check_weights(rows: &[IndexMap<String, f64>], weights: Option<&[f64]>) -> Result<()> {
    let Some(weights) = weights else {
        return Ok(());
    };
    if weights.len() != rows.len() {
        return Err(Error::value(format!(
            "{} weights for {} rows: give one per row",
            weights.len(),
            rows.len()
        )));
    }
    if let Some((i, w)) = weights
        .iter()
        .enumerate()
        .find(|(_, w)| !(w.is_finite() && **w >= 0.0))
    {
        return Err(Error::value(format!(
            "row {i}'s weight is {}: a weight is a finite number, not negative",
            py_float_repr(*w)
        )));
    }
    Ok(())
}

/// Per part, per parameter, per point: the quantity's parameters as written.
fn start_params(q: &Quantity) -> Vec<Vec<Vec<f64>>> {
    q.parts.iter().map(|p| p.values.clone()).collect()
}

/// `log pdf(y)` of `d`, and its derivative by each of its parameters (in
/// [`Family::params`]' order; a normal's `low` and `high` get 0).
fn log_pdf_grad(d: &Dist, y: f64) -> (f64, [f64; 4]) {
    let (low, high) = d.support();
    if !(low <= y && y <= high) {
        return (f64::NEG_INFINITY, [0.0; 4]);
    }
    match *d {
        Dist::Uniform { low, high } => {
            let w = high - low;
            if !(w > 0.0) {
                return (f64::NEG_INFINITY, [0.0; 4]);
            }
            (-w.ln(), [1.0 / w, -1.0 / w, 0.0, 0.0])
        }
        Dist::Triangular { low, mode, high } => {
            let w = high - low;
            if !(w > 0.0) {
                return (f64::NEG_INFINITY, [0.0; 4]);
            }
            let left = y < mode || (y == mode && mode == high);
            if left {
                let (a, b) = (y - low, mode - low);
                if !(a > 0.0 && b > 0.0) {
                    return (f64::NEG_INFINITY, [0.0; 4]);
                }
                let lp = 2f64.ln() + a.ln() - w.ln() - b.ln();
                (lp, [-1.0 / a + 1.0 / w + 1.0 / b, -1.0 / b, -1.0 / w, 0.0])
            } else {
                let (a, b) = (high - y, high - mode);
                if !(a > 0.0 && b > 0.0) {
                    return (f64::NEG_INFINITY, [0.0; 4]);
                }
                let lp = 2f64.ln() + a.ln() - w.ln() - b.ln();
                (lp, [1.0 / w, 1.0 / b, 1.0 / a - 1.0 / w - 1.0 / b, 0.0])
            }
        }
        Dist::Normal {
            mean,
            sd,
            low,
            high,
        } => {
            if !(sd > 0.0) {
                return (f64::NEG_INFINITY, [0.0; 4]);
            }
            let (a, b, z) = ((low - mean) / sd, (high - mean) / sd, (y - mean) / sd);
            // In logarithms: the mass between the ends may be too small for
            // a number, the mean being far outside them.
            let s = Truncated::new(mean, sd, low, high);
            if !s.ln_mass.is_finite() {
                return (f64::NEG_INFINITY, [0.0; 4]);
            }
            // density(±inf) / mass is 0, and so must z times it be there.
            let (da, db) = (s.density_share(a), s.density_share(b));
            let times = |z: f64, d: f64| if z.is_finite() { z * d } else { 0.0 };
            let lp = s.ln_pdf(z) - sd.ln();
            let d_mean = z / sd - (da - db) / sd;
            let d_sd = (z * z - 1.0) / sd - (times(a, da) - times(b, db)) / sd;
            (lp, [d_mean, d_sd, 0.0, 0.0])
        }
    }
}

/// One row's term of the likelihood: each part's distribution there (into
/// `dists`), the part deciding the quantity's density at `y`, and that
/// part's `log pdf(y)` and its gradient ([`log_pdf_grad`]).
fn row_term(
    q: &Quantity,
    params: &[Vec<Vec<f64>>],
    corners: &[Vec<(usize, f64)>],
    y: f64,
    dists: &mut Vec<Dist>,
) -> (usize, f64, [f64; 4]) {
    dists.clear();
    dists.extend(
        q.parts
            .iter()
            .zip(params)
            .zip(corners)
            .map(|((part, p), c)| part.dist_with(p, c)),
    );
    let (j, _) = q.combine.deciding(dists.iter().map(|d| d.cdf(y)));
    let (lp, g) = log_pdf_grad(&dists[j], y);
    (j, lp, g)
}

/// How well `q` with `params` matches `data` ([`Calibration`]).
fn calibration_of(q: &Quantity, params: &[Vec<Vec<f64>>], data: &Data) -> Calibration {
    let total: f64 = data.weight.iter().sum();
    let mut dists = Vec::with_capacity(q.parts.len());
    let mut us: Vec<(f64, f64)> = Vec::with_capacity(data.y.len());
    let (mut ll, mut in_band, mut outside) = (0.0, 0.0, 0.0);
    let mut histogram = vec![0.0; 10];
    for (r, &y) in data.y.iter().enumerate() {
        let w = data.weight[r];
        let (_, lp, _) = row_term(q, params, &data.corners[r], y, &mut dists);
        let (_, u) = q.combine.deciding(dists.iter().map(|d| d.cdf(y)));
        ll += w * lp;
        if !lp.is_finite() {
            outside += w;
        }
        if (0.05..=0.95).contains(&u) {
            in_band += w;
        }
        histogram[((u * 10.0) as usize).min(9)] += w;
        us.push((u, w));
    }
    us.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut ks = 0.0f64;
    let mut below = 0.0;
    for (u, w) in us {
        ks = ks.max((below / total - u).abs());
        below += w;
        ks = ks.max((below / total - u).abs());
    }
    let share = |x: f64| if total > 0.0 { x / total } else { 0.0 };
    Calibration {
        rows: data.y.len(),
        mean_log_likelihood: share(ll),
        in_band: share(in_band),
        outside: share(outside),
        ks,
        histogram: histogram.into_iter().map(share).collect(),
    }
}

/// The weighted log-likelihood of `data` under `q` with `params`.
fn log_likelihood_of(q: &Quantity, params: &[Vec<Vec<f64>>], data: &Data) -> f64 {
    let mut dists = Vec::with_capacity(q.parts.len());
    data.y
        .iter()
        .enumerate()
        .map(|(r, &y)| data.weight[r] * row_term(q, params, &data.corners[r], y, &mut dists).1)
        .sum()
}

/// The barrier's weights, stage by stage, down to the likelihood alone
/// (see [`Problem::eval`]).
const BARRIER: [f64; 9] = [1e-1, 1e-2, 1e-3, 1e-4, 1e-5, 1e-6, 1e-7, 1e-8, 0.0];

/// One quantity's fitting problem, in units of `scale` (the spread of its
/// observed values): the variables are, per part, per estimated parameter,
/// the value at each point with data.
struct Problem<'a> {
    p: &'a Prepared<'a>,
    /// (part, parameter, point) of each variable.
    vars: Vec<(usize, usize, usize)>,
    scale: f64,
    floor: f64,
    total_weight: f64,
    /// The parameters at the last `x`: the start's, the variables'
    /// overwritten -- kept, so an evaluation allocates nothing for them.
    params: RefCell<Vec<Vec<Vec<f64>>>>,
    /// The gradient by the parameters, likewise kept.
    grad: RefCell<Vec<Vec<Vec<f64>>>>,
}

/// How a variable maps to the parameter it sets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Var {
    /// The parameter, in units of the scale: `low`, `high`, `mean`.
    Linear,
    /// A triangular's `mode`, as the logit of its share of `low..high`: so
    /// it is never outside them.
    ModeLogit,
    /// A normal's `sd`, as the log of it in units of the scale: so it is
    /// never negative.
    LogSd,
}

fn var_kind(family: Family, k: usize) -> Var {
    match (family, k) {
        (Family::Triangular, 1) => Var::ModeLogit,
        (Family::Normal, 1) => Var::LogSd,
        _ => Var::Linear,
    }
}

fn sigmoid(s: f64) -> f64 {
    1.0 / (1.0 + (-s).exp())
}

impl Problem<'_> {
    fn family(&self, i: usize) -> Family {
        self.p.q.parts[i].family
    }

    /// Write the parameters at `x` into `p`.
    fn set_params(&self, x: &[f64], p: &mut [Vec<Vec<f64>>]) {
        for (&(i, k, n), v) in self.vars.iter().zip(x) {
            match var_kind(self.family(i), k) {
                Var::Linear => p[i][k][n] = v * self.scale,
                Var::LogSd => p[i][k][n] = v.exp() * self.scale,
                Var::ModeLogit => {}
            }
        }
        // A mode after the range it is a share of.
        for (&(i, k, n), v) in self.vars.iter().zip(x) {
            if var_kind(self.family(i), k) == Var::ModeLogit {
                let (low, high) = (p[i][0][n], p[i][2][n]);
                p[i][1][n] = low + sigmoid(*v) * (high - low);
            }
        }
    }

    /// The parameters at `x`.
    fn params(&self, x: &[f64]) -> Vec<Vec<Vec<f64>>> {
        let mut p = self.params.borrow().clone();
        self.set_params(x, &mut p);
        p
    }

    fn start(&self) -> Vec<f64> {
        let base = self.params.borrow();
        self.vars
            .iter()
            .map(|&(i, k, n)| {
                let b = &base[i];
                match var_kind(self.family(i), k) {
                    Var::Linear => b[k][n] / self.scale,
                    Var::LogSd => (b[k][n] / self.scale).ln(),
                    Var::ModeLogit => {
                        let t = ((b[1][n] - b[0][n]) / (b[2][n] - b[0][n])).clamp(1e-3, 1.0 - 1e-3);
                        (t / (1.0 - t)).ln()
                    }
                }
            })
            .collect()
    }

    /// The gradient by the variables, of a function whose gradient by the
    /// parameters is `grad`, at `x` (whose parameters are `params`).
    fn chain(&self, x: &[f64], params: &[Vec<Vec<f64>>], grad: &[Vec<Vec<f64>>]) -> Vec<f64> {
        self.vars
            .iter()
            .zip(x)
            .map(|(&(i, k, n), v)| {
                let (p, g) = (&params[i], &grad[i]);
                match (var_kind(self.family(i), k), self.family(i)) {
                    // A triangular's mode, a share of its range, moves with it.
                    (Var::Linear, Family::Triangular) if self.p.free[i][1][n] => {
                        let t = (p[1][n] - p[0][n]) / (p[2][n] - p[0][n]);
                        let share = if k == 0 { 1.0 - t } else { t };
                        (g[k][n] + g[1][n] * share) * self.scale
                    }
                    (Var::Linear, _) => g[k][n] * self.scale,
                    (Var::LogSd, _) => g[k][n] * p[k][n],
                    (Var::ModeLogit, _) => {
                        let t = sigmoid(*v);
                        g[1][n] * (p[2][n] - p[0][n]) * t * (1.0 - t)
                    }
                }
            })
            .collect()
    }

    /// Whether every distribution at a point with data is valid, and wide
    /// enough where the fit moves its width.
    fn valid(&self, params: &[Vec<Vec<f64>>]) -> bool {
        for (i, part) in self.p.q.parts.iter().enumerate() {
            let f = &self.p.free[i];
            let width: &[usize] = match part.family.fitted_range() {
                Some((lo, hi)) => &[lo, hi],
                None => &[1],
            };
            for &n in &self.p.touched[i] {
                let mut p = [0.0; 4];
                for (v, column) in p.iter_mut().zip(&params[i]) {
                    *v = column[n];
                }
                let p = &p[..params[i].len()];
                if p.iter().any(|v| !v.is_finite()) || part.family.dist(p).check().is_err() {
                    return false;
                }
                if width.iter().any(|&k| f[k][n])
                    && part.family.spread(p) < least_spread(part.family, p, self.floor)
                {
                    return false;
                }
            }
        }
        true
    }

    /// The negated mean log-likelihood per unit of weight at `x`, less `mu`
    /// times the barrier keeping each row inside the ranges it must lie in,
    /// and its gradient; `None` where a distribution is invalid or a row
    /// impossible.
    ///
    /// The barrier is `log(y - low)` and `log(high - y)` (flattened to 0
    /// one scale away from the edge) of the deciding part, and of every
    /// other part the side that bounds the quantity: its `high` under `min`
    /// (the least of the parts' values is at most each one's), its `low`
    /// under `max`. Only a uniform's and a triangular's range is estimated,
    /// so only theirs take part.
    fn eval(&self, x: &[f64], mu: f64) -> Option<(f64, Vec<f64>)> {
        let mut params = self.params.borrow_mut();
        self.set_params(x, &mut params);
        if !self.valid(&params) {
            return None;
        }
        let mut grad = self.grad.borrow_mut();
        for (i, g) in grad.iter_mut().enumerate() {
            for column in g.iter_mut() {
                for &n in &self.p.touched[i] {
                    column[n] = 0.0;
                }
            }
        }
        let (q, data) = (self.p.q, &self.p.data);
        let mut total = 0.0;
        let mut dists: Vec<Dist> = Vec::with_capacity(q.parts.len());
        for (r, &y) in data.y.iter().enumerate() {
            let corners = &data.corners[r];
            let (j, lp, g) = row_term(q, &params, corners, y, &mut dists);
            if !lp.is_finite() {
                return None;
            }
            let w = data.weight[r];
            total += w * lp;
            for &(n, cw) in &corners[j] {
                for (k, gk) in g.iter().take(grad[j].len()).enumerate() {
                    grad[j][k][n] += w * cw * gk;
                }
            }
            if mu == 0.0 {
                continue;
            }
            for (i, d) in dists.iter().enumerate() {
                let Some((lo, hi)) = q.parts[i].family.fitted_range() else {
                    continue;
                };
                let (low, high) = d.support();
                let sides = [
                    (lo, y - low, -1.0, i == j || q.combine == Combine::Max),
                    (hi, high - y, 1.0, i == j || q.combine == Combine::Min),
                ];
                for (k, gap, sign, applies) in sides {
                    if !applies {
                        continue;
                    }
                    if !(gap > 0.0) {
                        return None;
                    }
                    // log(g) - g + 1 in units of the scale, 0 from g = 1 on:
                    // it holds a row off the edge, and lets a part that does
                    // not decide (so has no likelihood to hold it) rest.
                    let g = gap / self.scale;
                    if g < 1.0 {
                        total += w * mu * (g.ln() - g + 1.0);
                        for &(n, cw) in &corners[i] {
                            grad[i][k][n] += w * mu * cw * sign * (1.0 / gap - 1.0 / self.scale);
                        }
                    }
                }
            }
        }
        let norm = self.total_weight;
        let g = self
            .chain(x, &params, &grad)
            .into_iter()
            .map(|v| -v / norm)
            .collect();
        Some((-total / norm, g))
    }
}

/// The weighted standard deviation of `y` (or, when the values are all one,
/// the size of their values, or 1): the unit a quantity is fitted in.
fn scale_of(data: &Data) -> f64 {
    let size = data.y.iter().fold(0.0f64, |m, y| m.max(y.abs()));
    if data.y.windows(2).all(|w| w[0] == w[1]) {
        return if size > 0.0 { size } else { 1.0 };
    }
    let total: f64 = data.weight.iter().sum();
    let mean = data
        .y
        .iter()
        .zip(&data.weight)
        .map(|(y, w)| y * w)
        .sum::<f64>()
        / total;
    let var = data
        .y
        .iter()
        .zip(&data.weight)
        .map(|(y, w)| w * (y - mean).powi(2))
        .sum::<f64>()
        / total;
    var.sqrt()
}

fn fit_quantity(
    prepared: Prepared,
    options: &FitOptions,
    progress: &mut dyn FnMut(f64),
) -> Result<(Vec<Vec<Vec<f64>>>, QuantityFit)> {
    let (q, data) = (prepared.q, &prepared.data);
    let before = log_likelihood_of(q, &start_params(q), data);
    let scale = scale_of(data);
    let floor = options.min_spread * scale;
    let make = |base: Vec<Vec<Vec<f64>>>| {
        let grad = base
            .iter()
            .map(|p| p.iter().map(|c| vec![0.0; c.len()]).collect())
            .collect();
        Problem {
            p: &prepared,
            vars: prepared.vars(),
            scale,
            floor,
            total_weight: data.weight.iter().sum(),
            params: RefCell::new(base),
            grad: RefCell::new(grad),
        }
    };
    let stages = prepared.stages();
    // Rules under which every row is possible start as they are; others
    // are widened until it is.
    let mut problem = make(feasible_start(&prepared, floor, before.is_finite())?);
    let mut x0 = problem.start();
    let mut first = problem.eval(&x0, stages[0]);
    if first.is_none() && before.is_finite() {
        // A row on a range's end is possible, but the barrier holds rows
        // strictly inside: widen as for an impossible one.
        problem = make(feasible_start(&prepared, floor, false)?);
        x0 = problem.start();
        first = problem.eval(&x0, stages[0]);
    }
    let x0_kept = x0.clone();
    let Some(first) = first else {
        return Err(Error::value(format!(
            "{}: {}: cannot start the fit: a row lies outside the range of a \
             distribution, where its low or high is held (fixed)",
            q.source, q.name
        )));
    };
    // Interior point: the barrier's weight lowered stage by stage, each
    // stage starting where the last ended, then the likelihood alone.
    let (mut x, mut iterations, mut converged) = (x0, 0, true);
    let mut first = Some(first);
    let n = stages.len() as f64;
    for (stage, &mu) in stages.iter().enumerate() {
        // Within a stage: how many iterations it takes is not known ahead,
        // so its share fills in as 1 - exp(-iterations / 40).
        let mut tell = |it: usize| progress((stage as f64 + 1.0 - (-(it as f64) / 40.0).exp()) / n);
        let (xs, its, done) = minimize(&problem, x, first.take(), mu, options, &mut tell);
        progress((stage as f64 + 1.0) / n);
        x = xs;
        iterations += its;
        converged &= done;
    }
    // The values as the fitted file writes them, and the report says: a
    // range estimated rounded outwards ([`outwards`]).
    let written = |mut p: Vec<Vec<Vec<f64>>>| {
        for &(i, k, n) in &problem.vars {
            if let Some((lo, hi)) = q.parts[i].family.fitted_range() {
                if k == lo || k == hi {
                    p[i][k][n] = outwards(p[i][k][n], k == hi);
                }
            }
        }
        p
    };
    let mut params = written(problem.params(&x));
    let mut after = log_likelihood_of(q, &params, data);
    // An ascent ends no lower than it starts; with several parts or a
    // barrier it may reach a lesser maximum than the start's own -- then the
    // start stands.
    let start = written(problem.params(&x0_kept));
    let at_start = log_likelihood_of(q, &start, data);
    if at_start > after {
        (params, after) = (start, at_start);
    }
    // Nor below the rules as written, where the start was widened from
    // them (a row on a range's end).
    if before > after {
        (params, after) = (start_params(q), before);
    }
    let report = QuantityFit {
        calibration_before: calibration_of(q, &start_params(q), data),
        calibration: calibration_of(q, &params, data),
        name: q.name.clone(),
        rows: data.y.len(),
        skipped: data.skipped,
        log_likelihood_before: before,
        log_likelihood: after,
        iterations,
        converged,
        parts: q
            .parts
            .iter()
            .enumerate()
            .map(|(i, p)| PartFit {
                name: p.name.clone(),
                points: p.size(),
                points_with_data: prepared.touched[i].len(),
                estimated: problem.vars.iter().filter(|v| v.0 == i).count(),
            })
            .collect(),
    };
    Ok((params, report))
}

/// The least spread a distribution of parameters `p` may have at a point
/// with data: `floor`, or for a normal at most what its range allows -- a
/// normal truncated to a range is never wider than a uniform over it, whose
/// interquartile range is half the range's.
fn least_spread(family: Family, p: &[f64], floor: f64) -> f64 {
    match family {
        Family::Normal => floor.min(0.9 * (p[3] - p[2]) / 2.0 / 1.348_979_500_392_163_4),
        _ => floor,
    }
}

/// The rules' parameters, moved where needed so every row is possible and
/// every distribution at a point with data at least `floor` wide: unless
/// every row is possible already (`possible`), a uniform's or a
/// triangular's range widened past the values of the rows near each point
/// (each point's `low` below the least of them, `high` above the greatest --
/// so the range interpolated at a row, a weighted mean of the points', holds
/// the row's value); a normal's `sd` raised to `floor`.
fn feasible_start(prepared: &Prepared, floor: f64, possible: bool) -> Result<Vec<Vec<Vec<f64>>>> {
    let (q, data) = (prepared.q, &prepared.data);
    let mut params = start_params(q);
    for (i, part) in q.parts.iter().enumerate() {
        let (free, touched) = (&prepared.free[i], &prepared.touched[i]);
        let p = &mut params[i];
        match part.family.fitted_range() {
            Some(_) if possible => {}
            Some((lo, hi)) => {
                let size = part.size();
                let mut least = vec![f64::INFINITY; size];
                let mut most = vec![f64::NEG_INFINITY; size];
                for (r, &y) in data.y.iter().enumerate() {
                    for &(n, _) in &data.corners[r][i] {
                        least[n] = least[n].min(y);
                        most[n] = most[n].max(y);
                    }
                }
                for &n in touched {
                    if free[lo][n] {
                        p[lo][n] = p[lo][n].min(least[n] - floor);
                    }
                    if free[hi][n] {
                        p[hi][n] = p[hi][n].max(most[n] + floor);
                    }
                }
            }
            None => {
                for &n in touched {
                    if free[1][n] {
                        // Wide enough as truncated, which an end makes
                        // narrower than sd: sd raised until it is.
                        p[1][n] = p[1][n].max(floor);
                        for _ in 0..200 {
                            let at: Vec<f64> = p.iter().map(|c| c[n]).collect();
                            if part.family.spread(&at) >= least_spread(part.family, &at, floor) {
                                break;
                            }
                            p[1][n] *= 2.0;
                        }
                    }
                }
                for (r, &y) in data.y.iter().enumerate() {
                    let (low, high) = part.dist_with(p, &data.corners[r][i]).support();
                    if !(low <= y && y <= high) {
                        return Err(Error::value(format!(
                            "{}: {}.{}: a row's {} ({}) lies outside the normal \
                             distribution's truncation there ({}..{}), which a fit holds: \
                             widen its low and high",
                            q.source,
                            q.name,
                            part.name,
                            q.name,
                            py_float_repr(y),
                            py_float_repr(low),
                            py_float_repr(high)
                        )));
                    }
                }
            }
        }
    }
    Ok(params)
}

/// L-BFGS with a backtracking line search that refuses steps out of the
/// feasible set: `x` minimizing `problem.eval` (whose value and gradient at
/// `x` may be given, `first`), the iterations taken, and whether it
/// converged.
fn minimize(
    problem: &Problem,
    mut x: Vec<f64>,
    first: Option<(f64, Vec<f64>)>,
    mu: f64,
    options: &FitOptions,
    progress: &mut dyn FnMut(usize),
) -> (Vec<f64>, usize, bool) {
    const HISTORY: usize = 10;
    const ARMIJO: f64 = 1e-4;
    const HALVINGS: usize = 60;
    const STALLS: usize = 3;
    if x.is_empty() {
        return (x, 0, true);
    }
    let (mut f, mut g) = first
        .or_else(|| problem.eval(&x, mu))
        .expect("each stage starts where the last ended, inside");
    let mut history: Vec<(Vec<f64>, Vec<f64>, f64)> = Vec::new();
    // The first step of a steepest descent: at most this far, in units of
    // the quantity's spread.
    let mut reach = 0.1;
    let mut stalls = 0;
    for it in 0..options.max_iterations {
        progress(it);
        let gmax = g.iter().fold(0.0f64, |m, v| m.max(v.abs()));
        if gmax < 1e-12 {
            return (x, it, true);
        }
        let mut d = direction(&g, &history);
        let mut slope = dot(&g, &d);
        let steepest = history.is_empty() || !(slope < 0.0);
        if steepest {
            history.clear();
            d = g.iter().map(|v| -v * reach / gmax).collect();
            slope = dot(&g, &d);
        }
        let mut step = 1.0;
        let mut accepted = None;
        for _ in 0..HALVINGS {
            let xn: Vec<f64> = x.iter().zip(&d).map(|(a, b)| a + step * b).collect();
            if let Some((fn_, gn)) = problem.eval(&xn, mu) {
                if fn_ <= f + ARMIJO * step * slope {
                    accepted = Some((xn, fn_, gn));
                    break;
                }
            }
            step *= 0.5;
        }
        let Some((xn, fn_, gn)) = accepted else {
            if !steepest {
                // The history misleads here: start it over.
                history.clear();
                continue;
            }
            // No step down the gradient improves it: a maximum, within
            // rounding.
            return (x, it, true);
        };
        if steepest {
            reach = if step == 1.0 {
                reach * 2.0
            } else {
                reach * step
            };
        }
        let s: Vec<f64> = xn.iter().zip(&x).map(|(a, b)| a - b).collect();
        let yv: Vec<f64> = gn.iter().zip(&g).map(|(a, b)| a - b).collect();
        let sy = dot(&s, &yv);
        if sy > 1e-12 * dot(&s, &s).sqrt() * dot(&yv, &yv).sqrt() && sy > 0.0 {
            if history.len() == HISTORY {
                history.remove(0);
            }
            history.push((s, yv, 1.0 / sy));
        }
        let gain = f - fn_;
        x = xn;
        f = fn_;
        g = gn;
        if gain < options.tolerance {
            // A small gain from a step cut back far says only that the
            // direction was poor: converged when it holds a few times over,
            // the history started over in between.
            stalls += 1;
            if stalls >= STALLS || step >= 0.5 && !steepest {
                return (x, it + 1, true);
            }
            history.clear();
            reach = reach.max(0.1);
        } else {
            stalls = 0;
        }
    }
    (x, options.max_iterations, false)
}

/// The L-BFGS direction: minus the inverse Hessian the history estimates,
/// times `g` (the two-loop recursion).
fn direction(g: &[f64], history: &[(Vec<f64>, Vec<f64>, f64)]) -> Vec<f64> {
    let mut q = g.to_vec();
    let mut alphas = Vec::with_capacity(history.len());
    for (s, y, rho) in history.iter().rev() {
        let a = rho * dot(s, &q);
        for (qi, yi) in q.iter_mut().zip(y) {
            *qi -= a * yi;
        }
        alphas.push(a);
    }
    if let Some((s, y, _)) = history.last() {
        let gamma = dot(s, y) / dot(y, y);
        for qi in &mut q {
            *qi *= gamma;
        }
    }
    for ((s, y, rho), a) in history.iter().zip(alphas.iter().rev()) {
        let b = rho * dot(y, &q);
        for (qi, si) in q.iter_mut().zip(s) {
            *qi += (a - b) * si;
        }
    }
    q.iter().map(|v| -v).collect()
}

fn dot(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// The fitted parts, a rule file's `quantities:` section: each fitted
/// part's distribution, a parameter one number where it is one everywhere.
/// The `basis:` of a fitted part: the data it was fitted from, how well the
/// fitted quantity matches it, who fitted and when.
fn fitted_basis(report: &QuantityFit, options: &FitOptions) -> serde_json::Value {
    let mut data = serde_json::Map::new();
    data.insert("rows".into(), report.rows.into());
    if !options.data_files.is_empty() {
        data.insert(
            "files".into(),
            options
                .data_files
                .iter()
                .map(|(file, sha256)| serde_json::json!({"file": file, "sha256": sha256}))
                .collect(),
        );
    }
    let c = &report.calibration;
    let mut basis = serde_json::json!({
        "kind": "fitted",
        "data": data,
        "calibration": {"in_band": c.in_band, "ks": c.ks, "outside": c.outside},
    });
    if !options.by.is_empty() {
        basis["by"] = options.by.clone().into();
    }
    if !options.date.is_empty() {
        basis["date"] = options.date.clone().into();
    }
    basis
}

fn quantities_text(
    fitted: &[(&Quantity, Vec<Vec<Vec<f64>>>)],
    reports: &[QuantityFit],
    options: &FitOptions,
) -> String {
    if fitted.is_empty() {
        return "quantities: {}\n".into();
    }
    let mut out = String::from("quantities:\n");
    for ((q, params), report) in fitted.iter().zip(reports) {
        // Names quoted: a name such as `true` or `null` is a string.
        out.push_str(&format!("  {}:\n    parts:\n", quoted(&q.name)));
        for (part, p) in q.parts.iter().zip(params) {
            // The points too, as the rules have them (from an extended file,
            // say), so the file says the shape its values have.
            let at: Vec<String> = part
                .axes
                .iter()
                .map(|a| {
                    if a.categorical() {
                        let labels: Vec<String> = a.labels.iter().map(|l| quoted(l)).collect();
                        return format!("{}: [{}]", quoted(&a.input), labels.join(", "));
                    }
                    format!(
                        "{}: {}",
                        quoted(&a.input),
                        nested_list(&a.points, &[a.points.len()], &py_float_repr)
                    )
                })
                .collect();
            out.push_str(&format!(
                "      {}:\n        at: {{{}}}\n        {}:\n",
                quoted(&part.name),
                at.join(", "),
                part.family.name()
            ));
            // After the distribution, so it reads as the part's: a part a
            // fit moved now rests on the data. One it held whole keeps the
            // basis it has.
            let moved = report
                .parts
                .iter()
                .find(|p| p.name == part.name)
                .is_some_and(|p| p.estimated > 0);
            let shape: Vec<usize> = part.axes.iter().map(|a| a.points.len()).collect();
            for (k, column) in part.family.params().iter().zip(p) {
                let written = if column.windows(2).all(|w| w[0] == w[1]) {
                    py_float_repr(column[0])
                } else {
                    nested_list(column, &shape, &py_float_repr)
                };
                out.push_str(&format!("          {k}: {written}\n"));
            }
            if moved {
                out.push_str(&format!(
                    "        basis: {}\n",
                    fitted_basis(report, options)
                ));
            }
        }
    }
    out
}

/// `s` as a YAML string scalar (a JSON string is one).
fn quoted(s: &str) -> String {
    serde_json::to_string(s).expect("a string serializes")
}

/// `v` rounded to ten significant digits, up or down. A range a fit
/// estimates closes on the data: the rows at its ends sit on it. Rounded
/// outwards, it keeps them inside however a reader rounds it (to ten digits,
/// as the editor does).
fn outwards(v: f64, up: bool) -> f64 {
    if v == 0.0 || !v.is_finite() {
        return v;
    }
    let scale = 10f64.powi(9 - v.abs().log10().floor() as i32);
    let r = if up {
        (v * scale).ceil()
    } else {
        (v * scale).floor()
    } / scale;
    // Still past `v` once the division rounds.
    if up {
        r.max(v)
    } else {
        r.min(v)
    }
}

/// `values` (the last axis fastest) as nested YAML flow lists of `shape`,
/// each number written by `number`.
pub fn nested_list(values: &[f64], shape: &[usize], number: &dyn Fn(f64) -> String) -> String {
    match shape {
        [] | [_] => format!(
            "[{}]",
            values
                .iter()
                .map(|v| number(*v))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        [_, rest @ ..] => {
            let step: usize = rest.iter().product();
            if step == 0 {
                return "[]".into();
            }
            let rows: Vec<String> = values
                .chunks(step)
                .map(|c| nested_list(c, rest, number))
                .collect();
            format!("[{}]", rows.join(", "))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::{Rng, SeedableRng};
    use rand_chacha::ChaCha8Rng;

    fn row(pairs: &[(&str, f64)]) -> IndexMap<String, f64> {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    /// Draws `n` rows of `quantity` from `rules`, the input `input` uniform
    /// over `range`.
    fn draw(
        rules: &RuleSet,
        quantity: &str,
        input: &str,
        range: (f64, f64),
        n: usize,
        seed: u64,
    ) -> Vec<IndexMap<String, f64>> {
        let mut rng = ChaCha8Rng::seed_from_u64(seed);
        (0..n)
            .map(|_| {
                let x = rng.gen_range(range.0..=range.1);
                let p: f64 = rng.gen();
                let mut r = row(&[(input, x)]);
                let y = rules.quantile(quantity, &r, p).unwrap();
                r.insert(quantity.to_string(), y);
                r
            })
            .collect()
    }

    const NORMAL: &str = r#"version: 2
quantities:
  grip:
    parts:
      water:
        at: {water: [0, 100]}
        normal: {mean: [0.9, 0.5], sd: [0.05, 0.1], low: 0, high: 2}
"#;

    #[test]
    fn a_normal_part_recovers_its_parameters() {
        let truth = RuleSet::parse(NORMAL, "truth.yaml").unwrap();
        let rows = draw(&truth, "grip", "water", (0.0, 100.0), 4000, 1);
        let start = RuleSet::parse(
            &NORMAL.replace("[0.9, 0.5], sd: [0.05, 0.1]", "[0.7, 0.7], sd: [0.3, 0.3]"),
            "start.yaml",
        )
        .unwrap();
        let fit = start.fit(&rows, None, &FitOptions::default()).unwrap();
        let report = &fit.quantities[0];
        assert!(report.converged, "{report:?}");
        assert_eq!(report.rows, 4000);
        assert!(report.log_likelihood > report.log_likelihood_before);
        let part = &fit.rules.quantities["grip"].parts[0];
        let close = |got: f64, want: f64, tol: f64| {
            assert!((got - want).abs() < tol, "{got} vs {want}");
        };
        close(part.values[0][0], 0.9, 0.01);
        close(part.values[0][1], 0.5, 0.02);
        close(part.values[1][0], 0.05, 0.01);
        close(part.values[1][1], 0.1, 0.01);
        // The truncation is held.
        assert_eq!(part.values[2], vec![0.0, 0.0]);
        assert_eq!(part.values[3], vec![2.0, 2.0]);
        // The truth is about as likely as the fit, and no likelier.
        let truth_ll = truth.log_likelihood(&rows, None).unwrap()["grip"].0;
        assert!(report.log_likelihood >= truth_ll - 1e-6);
        assert!(report.log_likelihood - truth_ll < 10.0);
    }

    #[test]
    fn a_uniform_part_closes_on_the_data() {
        let text = r#"version: 2
quantities:
  grip:
    parts:
      water:
        at: {water: [0, 50, 100]}
        uniform: {low: [0.9, 0.65, 0.45], high: [1.1, 0.85, 0.65]}
"#;
        let truth = RuleSet::parse(text, "truth.yaml").unwrap();
        let rows = draw(&truth, "grip", "water", (0.0, 100.0), 3000, 2);
        // A start too narrow for the data: rows outside it.
        let start = RuleSet::parse(
            &text.replace(
                "[0.9, 0.65, 0.45], high: [1.1, 0.85, 0.65]",
                "[0.7, 0.7, 0.7], high: [0.72, 0.72, 0.72]",
            ),
            "start.yaml",
        )
        .unwrap();
        let fit = start.fit(&rows, None, &FitOptions::default()).unwrap();
        let report = &fit.quantities[0];
        assert_eq!(report.log_likelihood_before, f64::NEG_INFINITY);
        assert!(report.log_likelihood.is_finite());
        let part = &fit.rules.quantities["grip"].parts[0];
        for (got, want) in part.values[0].iter().zip([0.9, 0.65, 0.45]) {
            assert!((got - want).abs() < 0.02, "low {got} vs {want}");
        }
        for (got, want) in part.values[1].iter().zip([1.1, 0.85, 0.65]) {
            assert!((got - want).abs() < 0.02, "high {got} vs {want}");
        }
        // Every row lies within the fitted range.
        for r in &rows {
            let (lo, hi) = fit.rules.joined("grip", r).unwrap().support();
            assert!(lo <= r["grip"] && r["grip"] <= hi);
        }
    }

    #[test]
    fn a_triangular_part_recovers_its_mode() {
        let text = r#"version: 2
quantities:
  speed:
    parts:
      sight:
        at: {visibility: [0, 400]}
        triangular: {low: [5, 25], mode: [8, 35], high: [10, 45]}
"#;
        let truth = RuleSet::parse(text, "truth.yaml").unwrap();
        let rows = draw(&truth, "speed", "visibility", (0.0, 400.0), 5000, 3);
        let start = RuleSet::parse(
            &text.replace("mode: [8, 35]", "mode: [6, 40]"),
            "start.yaml",
        )
        .unwrap();
        let fit = start.fit(&rows, None, &FitOptions::default()).unwrap();
        let part = &fit.rules.quantities["speed"].parts[0];
        for (k, want) in [[5.0, 25.0], [8.0, 35.0], [10.0, 45.0]].iter().enumerate() {
            for (got, w) in part.values[k].iter().zip(want) {
                assert!((got - w).abs() < 0.6, "param {k}: {got} vs {w}");
            }
        }
    }

    #[test]
    fn a_chain_is_fitted_factor_by_factor() {
        let text = r#"version: 2
inputs:
  fog: {default: 0}
quantities:
  visibility:
    parts:
      fog:
        at: {fog: [0, 100]}
        normal: {mean: [300, 40], sd: [30, 10], low: 0, high: 500}
  speed:
    parts:
      sight:
        at: {visibility: [0, 400]}
        normal: {mean: [10, 40], sd: [2, 4], low: 0, high: 80}
"#;
        let truth = RuleSet::parse(text, "truth.yaml").unwrap();
        let mut rng = ChaCha8Rng::seed_from_u64(4);
        let mut rows = Vec::new();
        for i in 0..3000 {
            let fog: f64 = rng.gen_range(0.0..100.0);
            let qs = row(&[("visibility", rng.gen()), ("speed", rng.gen())]);
            let mut all = truth.sample(&row(&[("fog", fog)]), &qs).unwrap();
            all.insert("fog".into(), fog);
            // Some rows lose the visibility: they say nothing of either factor
            // reading or giving it... except the visibility's own, missing.
            if i % 3 == 0 {
                all.shift_remove("visibility");
            }
            rows.push(all);
        }
        let start = RuleSet::parse(
            &text
                .replace("[300, 40], sd: [30, 10]", "[200, 200], sd: [100, 100]")
                .replace("[10, 40], sd: [2, 4]", "[25, 25], sd: [10, 10]"),
            "start.yaml",
        )
        .unwrap();
        let fit = start.fit(&rows, None, &FitOptions::default()).unwrap();
        let names: Vec<&str> = fit.quantities.iter().map(|q| q.name.as_str()).collect();
        assert_eq!(names, ["visibility", "speed"]);
        assert_eq!(fit.quantities[0].rows, 2000);
        assert_eq!(fit.quantities[1].rows, 2000);
        assert_eq!(fit.quantities[1].skipped, 1000);
        let vis = &fit.rules.quantities["visibility"].parts[0];
        assert!((vis.values[0][0] - 300.0).abs() < 5.0, "{:?}", vis.values);
        assert!((vis.values[0][1] - 40.0).abs() < 3.0, "{:?}", vis.values);
        let speed = &fit.rules.quantities["speed"].parts[0];
        assert!(
            (speed.values[0][0] - 10.0).abs() < 1.5,
            "{:?}",
            speed.values
        );
        assert!(
            (speed.values[0][1] - 40.0).abs() < 1.5,
            "{:?}",
            speed.values
        );
    }

    #[test]
    fn parts_joined_by_min_are_fitted_through_the_deciding_part() {
        let text = r#"version: 2
quantities:
  grip:
    parts:
      water:
        at: {water: [0, 100]}
        uniform: {low: [0.8, 0.4], high: [1.0, 0.6]}
      ice:
        at: {cold: [0, 1]}
        uniform: {low: [0.9, 0.1], high: [1.1, 0.2]}
"#;
        let truth = RuleSet::parse(text, "truth.yaml").unwrap();
        let mut rng = ChaCha8Rng::seed_from_u64(5);
        let rows: Vec<_> = (0..2000)
            .map(|_| {
                let mut r = row(&[
                    ("water", rng.gen_range(0.0..100.0)),
                    ("cold", if rng.gen::<f64>() < 0.5 { 0.0 } else { 1.0 }),
                ]);
                let y = truth.quantile("grip", &r, rng.gen()).unwrap();
                r.insert("grip".into(), y);
                r
            })
            .collect();
        let start = RuleSet::parse(
            &text.replace(
                "low: [0.9, 0.1], high: [1.1, 0.2]",
                "low: [0.9, 0.0], high: [1.1, 0.5]",
            ),
            "s.yaml",
        )
        .unwrap();
        let before = start.log_likelihood(&rows, None).unwrap()["grip"].0;
        let fit = start.fit(&rows, None, &FitOptions::default()).unwrap();
        assert!(fit.quantities[0].log_likelihood > before);
        let ice = &fit.rules.quantities["grip"].parts[1];
        assert!((ice.values[0][1] - 0.1).abs() < 0.01, "{:?}", ice.values);
        assert!((ice.values[1][1] - 0.2).abs() < 0.01, "{:?}", ice.values);
    }

    #[test]
    fn a_fit_never_ends_below_where_it_starts() {
        // Three uniform parts joined by min, the rules as written already
        // possible for every row: the fit starts from them, not from ranges
        // widened, and ends no lower.
        let text = r#"version: 2
quantities:
  visibility:
    combine: min
    parts:
      fog:
        at: {fog: [0, 30, 70, 100]}
        uniform: {low: [250, 120, 40, 15], high: [400, 250, 100, 40]}
      rain:
        at: {rain: [0, 30, 70, 100]}
        uniform: {low: [250, 180, 100, 60], high: [400, 320, 220, 150]}
      light:
        at: {sun: [-6, 0]}
        uniform: {low: [50, 250], high: [150, 400]}
"#;
        let truth = RuleSet::parse(text, "v.yaml").unwrap();
        let mut rng = ChaCha8Rng::seed_from_u64(11);
        let rows: Vec<_> = (0..600)
            .map(|_| {
                let mut r = row(&[
                    ("fog", rng.gen_range(0.0..100.0)),
                    ("rain", rng.gen_range(0.0..100.0)),
                    ("sun", rng.gen_range(-6.0..0.0)),
                ]);
                let y = truth.quantile("visibility", &r, rng.gen()).unwrap();
                r.insert("visibility".into(), y);
                r
            })
            .collect();
        let fit = truth.fit(&rows, None, &FitOptions::default()).unwrap();
        let q = &fit.quantities[0];
        assert!(q.log_likelihood_before.is_finite());
        assert!(q.log_likelihood >= q.log_likelihood_before, "{q:?}");
    }

    #[test]
    fn a_point_without_data_keeps_its_value_and_the_overlay_reads_back() {
        let text = r#"version: 2
quantities:
  grip:
    parts:
      water:
        at: {water: [0, 50, 100]}
        normal: {mean: [0.9, 0.7, 0.5], sd: 0.1, low: 0, high: 2}
"#;
        let start = RuleSet::parse(text, "start.yaml").unwrap();
        let rows: Vec<_> = (0..200)
            .map(|i| {
                row(&[
                    ("water", (i % 50) as f64 / 2.0),
                    ("grip", 0.8 + 0.001 * (i % 7) as f64),
                ])
            })
            .collect();
        let fit = start.fit(&rows, None, &FitOptions::default()).unwrap();
        let part = &fit.rules.quantities["grip"].parts[0];
        assert_eq!(fit.quantities[0].parts[0].points_with_data, 2);
        assert_eq!(part.values[0][2], 0.5);
        assert_eq!(part.values[1][2], 0.1);
        let text = fit.text(Some("start.yaml"));
        assert!(
            text.starts_with("version: 2\nextends: \"start.yaml\"\n"),
            "{text}"
        );
        let again = start.with_overlay(&fit.text(None)).unwrap();
        assert_eq!(
            again.quantities["grip"].parts,
            fit.rules.quantities["grip"].parts
        );
    }

    #[test]
    fn rows_and_options_are_checked() {
        let start = RuleSet::parse(NORMAL, "n.yaml").unwrap();
        let rows = vec![row(&[("water", 10.0), ("grip", 3.0)])];
        let err = start
            .fit(&rows, None, &FitOptions::default())
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("outside the normal distribution's truncation"),
            "{err}"
        );
        let err = start
            .fit(&rows, Some(&[1.0, 2.0]), &FitOptions::default())
            .unwrap_err()
            .to_string();
        assert!(err.contains("2 weights for 1 rows"), "{err}");
        let options = FitOptions {
            quantities: Some(vec!["grip".into()]),
            ..FitOptions::default()
        };
        let err = start
            .fit(&[row(&[("grip", 1.0)])], None, &options)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("cannot fit grip: no row gives it and every value it reads (water)"),
            "{err}"
        );
        // Without naming it, a quantity no row observes is left as it is.
        let fit = start
            .fit(&[row(&[("water", 1.0)])], None, &FitOptions::default())
            .unwrap();
        assert!(fit.quantities.is_empty());
        assert_eq!(fit.rules.quantities, start.quantities);
    }

    fn options(fixed: &[&str]) -> FitOptions {
        FitOptions {
            fixed: fixed.iter().map(|s| s.to_string()).collect(),
            ..FitOptions::default()
        }
    }

    #[test]
    fn what_the_rules_hold_stays_as_written() {
        let truth = RuleSet::parse(NORMAL, "truth.yaml").unwrap();
        let rows = draw(&truth, "grip", "water", (0.0, 100.0), 3000, 6);
        let start = NORMAL.replace("[0.9, 0.5], sd: [0.05, 0.1]", "[0.7, 0.7], sd: [0.3, 0.3]");
        // The file holds sd everywhere, and the mean where water is 0.
        let held = RuleSet::parse(
            &format!("{start}        fixed: [sd, \"mean@water=0\"]\n"),
            "held.yaml",
        )
        .unwrap();
        let fit = held.fit(&rows, None, &FitOptions::default()).unwrap();
        let part = &fit.rules.quantities["grip"].parts[0];
        assert_eq!(part.values[1], vec![0.3, 0.3]);
        assert_eq!(part.values[0][0], 0.7);
        // The one value free moves to make up for those held (0.7 at
        // water 0 where the truth is 0.9): the best given them, not the truth.
        assert!(part.values[0][1] != 0.7, "{:?}", part.values);
        assert!(fit.quantities[0].log_likelihood > fit.quantities[0].log_likelihood_before);
        assert_eq!(fit.quantities[0].parts[0].estimated, 1);
        // What it holds survives the fitted overlay.
        assert_eq!(part.fixed, held.quantities["grip"].parts[0].fixed);
        // The same, held by the fit instead of the file.
        let free = RuleSet::parse(&start, "start.yaml").unwrap();
        let by_call = free
            .fit(
                &rows,
                None,
                &options(&["grip.water.sd", "grip.water.mean@water=0"]),
            )
            .unwrap();
        assert_eq!(
            by_call.rules.quantities["grip"].parts[0].values,
            part.values
        );
        // A point held whole; a part held whole; a quantity held whole.
        let at = free
            .fit(&rows, None, &options(&["grip.water@water=100"]))
            .unwrap();
        let v = &at.rules.quantities["grip"].parts[0].values;
        assert_eq!((v[0][1], v[1][1]), (0.7, 0.3));
        assert!(v[0][0] != 0.7 && v[1][0] != 0.3, "{v:?}");
        assert_eq!(at.quantities[0].parts[0].estimated, 2);
        for whole in ["grip", "grip.water"] {
            let none = free.fit(&rows, None, &options(&[whole])).unwrap();
            assert!(none.quantities.is_empty(), "{whole}");
            assert_eq!(
                none.rules.quantities["grip"].parts,
                free.quantities["grip"].parts
            );
        }
    }

    #[test]
    fn a_triangular_holds_its_mode_or_its_range() {
        let text = r#"version: 2
quantities:
  speed:
    parts:
      sight:
        at: {visibility: [0, 400]}
        triangular: {low: [5, 25], mode: [8, 35], high: [10, 45]}
"#;
        let truth = RuleSet::parse(text, "truth.yaml").unwrap();
        let rows = draw(&truth, "speed", "visibility", (0.0, 400.0), 3000, 8);
        let start =
            RuleSet::parse(&text.replace("mode: [8, 35]", "mode: [6, 40]"), "s.yaml").unwrap();
        // The mode held: the range is still fitted around it.
        let fit = start
            .fit(&rows, None, &options(&["speed.sight.mode"]))
            .unwrap();
        let v = &fit.rules.quantities["speed"].parts[0].values;
        assert_eq!(v[1], vec![6.0, 40.0]);
        assert!(fit.quantities[0].log_likelihood > fit.quantities[0].log_likelihood_before);
        // The range held: the mode alone is fitted, inside it.
        let fit = start
            .fit(
                &rows,
                None,
                &options(&["speed.sight.low", "speed.sight.high"]),
            )
            .unwrap();
        let v = &fit.rules.quantities["speed"].parts[0].values;
        assert_eq!(
            (v[0].clone(), v[2].clone()),
            (vec![5.0, 25.0], vec![10.0, 45.0])
        );
        assert!(
            (v[1][0] - 8.0).abs() < 0.5 && (v[1][1] - 35.0).abs() < 1.0,
            "{v:?}"
        );
        // A range held too narrow for the data cannot be fitted.
        let narrow =
            RuleSet::parse(&text.replace("high: [10, 45]", "high: [9, 40]"), "n.yaml").unwrap();
        let err = narrow
            .fit(&rows, None, &options(&["speed.sight.high"]))
            .unwrap_err()
            .to_string();
        assert!(err.contains("where its low or high is held"), "{err}");
    }

    #[test]
    fn a_fixed_entry_must_name_something() {
        let start = RuleSet::parse(NORMAL, "n.yaml").unwrap();
        let rows = vec![row(&[("water", 10.0), ("grip", 0.8)])];
        for (spec, says) in [
            ("nope", "fixed 'nope': unknown quantity 'nope'"),
            (
                "grip.ice",
                "fixed 'grip.ice': grip has no part 'ice'; it has water",
            ),
            ("grip.water.median", "normal has no parameter 'median'"),
            (
                "grip.water.mean@water=5",
                "water=5.0 is not one of its points",
            ),
            ("grip@water=0", "a point is named in a part"),
        ] {
            let err = start
                .fit(&rows, None, &options(&[spec]))
                .unwrap_err()
                .to_string();
            assert!(err.contains(says), "{spec}: {err}");
        }
    }

    #[test]
    fn progress_rises_to_the_end() {
        let truth = RuleSet::parse(NORMAL, "truth.yaml").unwrap();
        let rows = draw(&truth, "grip", "water", (0.0, 100.0), 500, 9);
        let mut seen: Vec<Progress> = Vec::new();
        let fit = truth
            .fit_with_progress(&rows, None, &FitOptions::default(), &mut |p| {
                seen.push(p.clone())
            })
            .unwrap();
        assert!(!seen.is_empty());
        assert!(seen
            .windows(2)
            .all(|w| w[0].fraction <= w[1].fraction + 1e-12));
        assert_eq!(seen[0].fraction, 0.0);
        assert_eq!(seen.last().unwrap().fraction, 1.0);
        assert!(seen.iter().all(|p| p.quantity == "grip" && p.count == 1));
        assert_eq!(fit.quantities.len(), 1);
    }

    #[test]
    fn json_data_reads_a_categorical_input_at_its_labels() {
        let set = RuleSet::parse(
            "version: 2\ninputs:\n  tod: {labels: [day, night]}\nquantities:\n  sun:\n    parts:\n      t:\n        at: {tod: [day, night]}\n        uniform: {low: [10, -30], high: [70, -6]}\n",
            "s.yaml",
        )
        .unwrap();
        let o = observations_from_json(
            r#"[{"tod": "night", "sun": -10}, {"tod": "", "sun": 20}]"#,
            &set,
        )
        .unwrap();
        assert_eq!(o.rows[0]["tod"], 1.0);
        assert!(!o.rows[1].contains_key("tod"));
        let e = observations_from_json(r#"[{"tod": "noon", "sun": 1}]"#, &set)
            .unwrap_err()
            .to_string();
        assert!(e.contains("not one of its labels day, night"), "{e}");
    }

    #[test]
    fn json_data_comes_as_rows_or_columns_and_may_have_holes() {
        let names = RuleSet::parse(
            "version: 2\nquantities:\n  grip:\n    parts:\n      w:\n        at: {water: [0, 100]}\n        uniform: {low: 0, high: 1}\n",
            "g.yaml",
        )
        .unwrap();
        let rows = r#"{"rows": [
            {"time": "12:00", "water": 70, "grip": 0.62, "dt": 0.5},
            {"water": "20"},
            {"grip": null, "water": 5},
            {}
        ], "weight": "dt"}"#;
        let o = observations_from_json(rows, &names).unwrap();
        assert_eq!(o.rows.len(), 4);
        assert_eq!(o.rows[0], row(&[("water", 70.0), ("grip", 0.62)]));
        assert_eq!(o.rows[1], row(&[("water", 20.0)]));
        assert_eq!(o.rows[2], row(&[("water", 5.0)]));
        assert_eq!(o.weights(), Some(&[0.5, 1.0, 1.0, 1.0][..]));
        assert_eq!(
            o.observed,
            IndexMap::from([("water".to_string(), 3), ("grip".to_string(), 1)])
        );
        let list = observations_from_json(r#"[{"water": 1, "grip": 0.9}]"#, &names).unwrap();
        assert_eq!(list.rows, vec![row(&[("water", 1.0), ("grip", 0.9)])]);
        assert_eq!(list.weights(), None);
        let columns =
            observations_from_json(r#"{"water": [1, 2], "grip": [0.9, null]}"#, &names).unwrap();
        assert_eq!(
            columns.rows,
            vec![
                row(&[("water", 1.0), ("grip", 0.9)]),
                row(&[("water", 2.0)])
            ]
        );
        for (bad, says) in [
            ("[1]", "row 0 is 1: a row is an object"),
            (
                r#"[{"grip": "wet"}]"#,
                "row 0: grip is \"wet\", not a number",
            ),
            (r#"{"water": [1], "grip": [1, 2]}"#, "not all of one length"),
            (r#"{"rows": [], "weights": "w"}"#, "unknown key \"weights\""),
            (
                r#"{"rows": [{"w": -1}], "weight": "w"}"#,
                "its weight w is negative",
            ),
            ("{", "not JSON"),
        ] {
            let err = observations_from_json(bad, &names).unwrap_err().to_string();
            assert!(err.contains(says), "{bad}: {err}");
        }
    }

    #[test]
    fn the_report_is_of_the_rules_as_written() {
        // Found by fuzzing: two parts joined by max whose distribution
        // functions tie at the row after the fit. Rounding the file's range
        // flipped which part decides, and the fitted rules' log-likelihood
        // was not the one reported.
        let text = "version: 2\nquantities:\n  q0:\n    combine: max\n    parts:\n      p0:\n        at: {a: [0.0]}\n        uniform: {low: [0.0], high: [2.0]}\n      p1:\n        at: {a: [0.0]}\n        uniform: {low: [1.382390089321256e-194], high: [1.0]}\n";
        let set = RuleSet::parse(text, "m.yaml").unwrap();
        let rows = [row(&[("a", 0.0), ("q0", 1.382390089321256e-194)])];
        let fit = set.fit(&rows, None, &FitOptions::default()).unwrap();
        let reported = fit.quantities[0].log_likelihood;
        assert_eq!(
            fit.rules.log_likelihood(&rows, None).unwrap()["q0"].0,
            reported
        );
        assert_eq!(
            fit.rules.calibration(&rows, None).unwrap()["q0"],
            fit.quantities[0].calibration
        );
        assert!(reported >= fit.quantities[0].log_likelihood_before);
    }

    #[test]
    fn a_fit_ends_no_lower_than_the_rules_as_written() {
        // Found by fuzzing: a row on a triangular's low end, its mode there
        // too. The barrier's start widens the range, and the fit ended below
        // the rules as written.
        let text = "version: 2\nquantities:\n  q0:\n    parts:\n      p0:\n        at: {a: [0.0]}\n        triangular: {low: [0.0], mode: [0.0], high: [0.015625]}\n";
        let set = RuleSet::parse(text, "t.yaml").unwrap();
        let fit = set
            .fit(
                &[row(&[("a", 0.0), ("q0", 0.0)])],
                None,
                &FitOptions::default(),
            )
            .unwrap();
        let q = &fit.quantities[0];
        assert!(q.log_likelihood >= q.log_likelihood_before, "{q:?}");
    }

    #[test]
    fn a_narrow_normal_at_its_end_starts_a_fit() {
        // Found in review: the start raised sd to the floor, but at the
        // range's end the truncated spread is 0.62 sd, below it, and the fit
        // refused to start. And a range too narrow for the floor caps it.
        for (text, y) in [
            (
                "normal: {mean: [0.0], sd: [1e-4], low: [0.0], high: [1.0]}",
                0.0,
            ),
            (
                "normal: {mean: [0.5], sd: [1e-4], low: [0.4999], high: [0.5001]}",
                0.5,
            ),
        ] {
            let text = format!("version: 2\nquantities:\n  q0:\n    parts:\n      p0:\n        at: {{a: [0.0]}}\n        {text}\n");
            let set = RuleSet::parse(&text, "n.yaml").unwrap();
            let fit = set
                .fit(
                    &[row(&[("a", 0.0), ("q0", y)])],
                    None,
                    &FitOptions::default(),
                )
                .unwrap_or_else(|e| panic!("{text}: {e}"));
            assert!(fit.quantities[0].log_likelihood.is_finite());
        }
    }

    #[test]
    fn a_normal_squeezed_against_its_range_is_held_wide_and_reads() {
        // Found by fuzzing: one row on a normal's low end. Its likelihood
        // grows without bound as the mean leaves the range (the distribution
        // a spike at the end), which min_spread must stop; and the fitted
        // distribution, its mean far outside, must still read.
        let text = "version: 2\nquantities:\n  q0:\n    parts:\n      p0:\n        at: {a: [0.0]}\n        normal: {mean: [0.0], sd: [1.0], low: [0.0], high: [1.0]}\n";
        let set = RuleSet::parse(text, "n.yaml").unwrap();
        let fit = set
            .fit(
                &[row(&[("a", 0.0), ("q0", 0.0)])],
                None,
                &FitOptions::default(),
            )
            .unwrap();
        let summary = fit.rules.evaluate("q0", &row(&[("a", 0.0)])).unwrap();
        assert!(summary.values().all(|v| v.is_finite()), "{summary:?}");
        let part = &fit.rules.quantities["q0"].parts[0];
        let p: Vec<f64> = part.values.iter().map(|c| c[0]).collect();
        // One value, all its own size: the scale is 1, the floor 1e-3.
        assert!(Family::Normal.spread(&p) >= 1e-3 * (1.0 - 1e-6), "{p:?}");
        assert!(fit.quantities[0].log_likelihood.is_finite());
    }

    #[test]
    fn a_row_on_a_range_end_starts_a_fit() {
        let text = "version: 2\nquantities:\n  grip:\n    parts:\n      water:\n        at: {water: [0, 1]}\n        uniform: {low: 0, high: 1}\n";
        let set = RuleSet::parse(text, "e.yaml").unwrap();
        let rows = vec![
            row(&[("water", 0.5), ("grip", 0.0)]),
            row(&[("water", 0.5), ("grip", 0.5)]),
        ];
        let fit = set.fit(&rows, None, &FitOptions::default()).unwrap();
        assert!(fit.quantities[0].log_likelihood >= fit.quantities[0].log_likelihood_before);
    }

    #[test]
    fn names_yaml_reads_otherwise_are_written_quoted() {
        let text = "version: 2\nquantities:\n  \"true\":\n    parts:\n      \"null\":\n        at: {water: [0, 100]}\n        normal: {mean: 0.7, sd: 0.3, low: 0, high: 2}\n";
        let set = RuleSet::parse(text, "q.yaml").unwrap();
        let rows: Vec<_> = (0..50)
            .map(|i| row(&[("water", (i * 2) as f64), ("true", 0.5 + 0.004 * i as f64)]))
            .collect();
        let fit = set.fit(&rows, None, &FitOptions::default()).unwrap();
        let text = fit.text(None);
        assert!(
            text.contains(
                "  \"true\":\n    parts:\n      \"null\":\n        at: {\"water\": [0.0, 100.0]}"
            ),
            "{text}"
        );
        assert_eq!(fit.quantities[0].name, "true");
    }

    #[test]
    fn a_fitted_range_is_written_outwards_and_keeps_every_row() {
        assert_eq!(outwards(0.123456789012, false), 0.1234567890);
        assert_eq!(outwards(0.123456789012, true), 0.1234567891);
        assert_eq!(outwards(-2.5, true), -2.5);
        let text = r#"version: 2
quantities:
  grip:
    parts:
      water:
        at: {water: [0, 100]}
        uniform: {low: [0.9, 0.5], high: [1.1, 0.7]}
"#;
        let truth = RuleSet::parse(text, "t.yaml").unwrap();
        let rows = draw(&truth, "grip", "water", (0.0, 100.0), 500, 13);
        let fit = truth.fit(&rows, None, &FitOptions::default()).unwrap();
        // Rounded to ten digits, as the editor writes numbers, it still
        // holds every row.
        let ten = |v: f64| format!("{:.9e}", v).parse::<f64>().unwrap();
        let mut part = fit.rules.quantities["grip"].parts[0].clone();
        for column in &mut part.values {
            for v in column.iter_mut() {
                *v = ten(*v);
            }
        }
        for r in &rows {
            let (lo, hi) = part.dist(&[r["water"]]).support();
            assert!(
                lo <= r["grip"] && r["grip"] <= hi,
                "{lo} {} {hi}",
                r["grip"]
            );
        }
    }

    #[test]
    fn calibration_says_how_well_rules_match() {
        let truth = RuleSet::parse(NORMAL, "truth.yaml").unwrap();
        let rows = draw(&truth, "grip", "water", (0.0, 100.0), 4000, 12);
        let good = &truth.calibration(&rows, None).unwrap()["grip"];
        assert_eq!(good.rows, 4000);
        assert!((good.in_band - 0.9).abs() < 0.02, "{good:?}");
        assert!(good.ks < 1.36 / (4000f64).sqrt() * 1.5, "{good:?}");
        assert!(
            good.histogram.iter().all(|h| (h - 0.1).abs() < 0.02),
            "{good:?}"
        );
        assert_eq!(good.outside, 0.0);
        // Too narrow: rows pile up in the tails, few in the band.
        let narrow = RuleSet::parse(
            &NORMAL.replace("sd: [0.05, 0.1]", "sd: [0.01, 0.02]"),
            "n.yaml",
        )
        .unwrap();
        let bad = &narrow.calibration(&rows, None).unwrap()["grip"];
        assert!(bad.in_band < 0.5 && bad.ks > 0.2, "{bad:?}");
        assert!(bad.histogram[0] + bad.histogram[9] > 0.5, "{bad:?}");
        assert!(bad.mean_log_likelihood < good.mean_log_likelihood);
        // A fit mends it, and says so.
        let fit = narrow.fit(&rows, None, &FitOptions::default()).unwrap();
        let q = &fit.quantities[0];
        assert_eq!(&q.calibration_before, bad);
        assert!((q.calibration.in_band - 0.9).abs() < 0.02, "{q:?}");
    }

    #[test]
    fn a_report_travels_as_json_minus_infinity_as_null() {
        let q = QuantityFit {
            name: "grip".into(),
            rows: 3,
            skipped: 1,
            log_likelihood_before: f64::NEG_INFINITY,
            log_likelihood: 2.5,
            iterations: 7,
            converged: true,
            parts: vec![PartFit {
                name: "water".into(),
                points: 2,
                points_with_data: 1,
                estimated: 2,
            }],
            calibration_before: Calibration {
                rows: 3,
                mean_log_likelihood: f64::NEG_INFINITY,
                in_band: 0.5,
                outside: 0.25,
                ks: 0.4,
                histogram: vec![0.1; 10],
            },
            calibration: Calibration {
                rows: 3,
                mean_log_likelihood: 0.8,
                in_band: 0.9,
                outside: 0.0,
                ks: 0.05,
                histogram: vec![0.1; 10],
            },
        };
        let text = serde_json::to_string(&q).unwrap();
        assert!(text.contains(r#""log_likelihood_before":null"#), "{text}");
        assert_eq!(serde_json::from_str::<QuantityFit>(&text).unwrap(), q);
    }

    #[test]
    fn usable_counts_the_rows_each_factor_reads() {
        let set = RuleSet::parse(
            r#"version: 2
quantities:
  visibility:
    parts: {fog: {at: {fog: [0, 100]}, uniform: {low: [250, 15], high: [400, 40]}}}
  speed:
    parts: {sight: {at: {visibility: [0, 400]}, uniform: {low: 0, high: 50}}}
"#,
            "u.yaml",
        )
        .unwrap();
        assert_eq!(set.data_names(), ["visibility", "speed", "fog"]);
        let rows = vec![
            row(&[("fog", 10.0), ("visibility", 300.0), ("speed", 30.0)]),
            row(&[("fog", 80.0), ("speed", 9.0)]),
            row(&[("visibility", 100.0)]),
        ];
        let usable = set.usable(&rows);
        assert_eq!(usable["visibility"], (1, 1));
        assert_eq!(usable["speed"], (1, 1));
    }

    #[test]
    fn the_gradient_is_the_likelihoods() {
        // Every family's analytic gradient against central differences.
        let dists = [
            Dist::Uniform {
                low: 0.1,
                high: 1.3,
            },
            Dist::Triangular {
                low: 0.0,
                mode: 0.4,
                high: 1.0,
            },
            Dist::Triangular {
                low: 0.0,
                mode: 0.9,
                high: 1.0,
            },
            Dist::Normal {
                mean: 0.3,
                sd: 0.2,
                low: 0.0,
                high: 1.0,
            },
            Dist::Normal {
                mean: -2.0,
                sd: 0.5,
                low: 0.0,
                high: 1.0,
            },
        ];
        for d in dists {
            let y = 0.6;
            let (_, g) = log_pdf_grad(&d, y);
            let p = d.params();
            for k in d.family().fitted_params() {
                let h = 1e-6;
                let mut up = p.clone();
                let mut down = p.clone();
                up[*k] += h;
                down[*k] -= h;
                let f = |q: &[f64]| log_pdf_grad(&d.family().dist(q), y).0;
                let numeric = (f(&up) - f(&down)) / (2.0 * h);
                assert!(
                    (numeric - g[*k]).abs() < 1e-5 * (1.0 + numeric.abs()),
                    "{d:?} param {k}: {numeric} vs {}",
                    g[*k]
                );
            }
        }
    }

    #[test]
    fn a_fitted_part_rests_on_the_data() {
        let truth = RuleSet::parse(NORMAL, "truth.yaml").unwrap();
        let rows = draw(&truth, "grip", "water", (0.0, 100.0), 500, 9);
        let two = format!(
            "{NORMAL}      held:\n        at: {{water: [0, 100]}}\n        uniform: {{low: 0, high: 2}}\n        fixed: true\n        basis: {{kind: expert}}\n"
        );
        let start = RuleSet::parse(&two, "start.yaml").unwrap();
        let options = FitOptions {
            data_files: vec![("drives.csv".into(), "abc".into())],
            by: "tests".into(),
            date: "2026-11-06".into(),
            ..FitOptions::default()
        };
        let fit = start.fit(&rows, None, &options).unwrap();
        let b = fit.rules.basis_of("grip", "water").unwrap();
        assert_eq!(b.kind, "fitted");
        let json = b.to_json();
        assert_eq!(json["data"]["rows"], 500);
        assert_eq!(json["data"]["files"][0]["sha256"], "abc");
        assert_eq!(json["by"], "tests");
        assert_eq!(json["date"], "2026-11-06");
        assert_eq!(
            json["calibration"]["in_band"].as_f64().unwrap(),
            fit.quantities[0].calibration.in_band
        );
        // The part it held keeps the basis written.
        assert_eq!(fit.rules.basis_of("grip", "held").unwrap().kind, "expert");
        // and the file says so, read back.
        let text = fit.text(None);
        let back = start
            .parse_over(
                &text,
                "fit.yaml",
                &Origin::Text,
                &mut Given(IndexMap::new()),
            )
            .unwrap();
        assert_eq!(back.basis_of("grip", "water").unwrap().kind, "fitted");
        // By default, this library is who fitted, and no date is written.
        let fit = start.fit(&rows, None, &FitOptions::default()).unwrap();
        let json = fit.rules.basis_of("grip", "water").unwrap().to_json();
        assert!(json["by"].as_str().unwrap().starts_with("openodd "));
        assert!(json.get("date").is_none());
    }
}
