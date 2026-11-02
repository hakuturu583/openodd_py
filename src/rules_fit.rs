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
//! normal's `sd` -- is kept at least `min_spread` times the spread of the
//! quantity's observed values: a likelihood grows without bound as a
//! distribution narrows onto one value, which is no estimate.

use indexmap::IndexMap;

use crate::error::{Error, Result};
use crate::rules::{density, phi, upper, Combine, Dist, Family, Hold, Part, Quantity, RuleSet};
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
}

impl Default for FitOptions {
    fn default() -> Self {
        FitOptions {
            quantities: None,
            max_iterations: 1000,
            tolerance: 1e-10,
            min_spread: 1e-3,
            fixed: Vec::new(),
        }
    }
}

/// What one part's fit touched.
#[derive(Debug, Clone, PartialEq)]
pub struct PartFit {
    pub name: String,
    /// The points it writes.
    pub points: usize,
    /// The points some row reads (interpolates from): the ones estimated.
    pub points_with_data: usize,
    /// The values estimated: each free parameter at each point with data
    /// (a value held, by the rules' `fixed` or the fit's, is not).
    pub estimated: usize,
}

/// One quantity's fit.
#[derive(Debug, Clone, PartialEq)]
pub struct QuantityFit {
    pub name: String,
    /// The rows that observe it and everything it reads.
    pub rows: usize,
    /// The rows that observe it but miss something it reads.
    pub skipped: usize,
    /// The log-likelihood of those rows (weighted) under the rules as they
    /// were: `-inf` when a row lies outside a distribution's range.
    pub log_likelihood_before: f64,
    /// ... and under the fitted rules.
    pub log_likelihood: f64,
    pub iterations: usize,
    /// Whether the fit stopped by converging, not at `max_iterations`.
    pub converged: bool,
    pub parts: Vec<PartFit>,
}

/// How far a fit has come ([`RuleSet::fit_with_progress`]).
#[derive(Debug, Clone, PartialEq)]
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

/// A fit: the fitted rules, and how each quantity was fitted.
#[derive(Debug, Clone)]
pub struct Fit {
    /// The rules, every fitted part's distribution replaced.
    pub rules: RuleSet,
    pub quantities: Vec<QuantityFit>,
    /// The fitted parts as a rule file that merges over the rules fitted
    /// (`version: 2` and `quantities:`; [`Fit::text`] adds an `extends`).
    pub overlay: String,
}

impl Fit {
    /// The fitted parts as a rule file; with `extends`, one extending that
    /// reference (the rules fitted), so it reads as the whole fitted set.
    pub fn text(&self, extends: Option<&str>) -> String {
        match extends {
            None => self.overlay.clone(),
            Some(e) => {
                let quoted = serde_json::to_string(e).expect("a string serializes");
                self.overlay.replacen(
                    "version: 2\n",
                    &format!("version: 2\nextends: {quoted}\n"),
                    1,
                )
            }
        }
    }
}

/// Rows of data, read from JSON ([`observations_from_json`]).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Observations {
    /// Per row, the values it gives of the names asked for.
    pub rows: Vec<IndexMap<String, f64>>,
    /// Per row, its weight, when the data names a weight column.
    pub weights: Option<Vec<f64>>,
    /// The weight column, if named.
    pub weight_column: Option<String>,
    /// Per name asked for, the rows that give it.
    pub observed: IndexMap<String, usize>,
}

/// The rows of a JSON data file, keeping the values of `names` (the
/// quantities and inputs the rules read; every other key -- a timestamp, an
/// id -- is ignored). The file is one of:
///
/// - `{"rows": [{name: value, ...}, ...], "weight": "<column>"}` (`weight`
///   optional: the column each row's weight is in, 1 where a row has none);
/// - a list of rows, `[{name: value, ...}, ...]`;
/// - columns, `{name: [value, ...], ...}`, each of one length.
///
/// A row need not give every name: one it leaves out, or gives as `null`,
/// is not observed there. A value is a number, or a string of one.
pub fn observations_from_json(text: &str, names: &[String]) -> Result<Observations> {
    use serde_json::Value as J;
    let doc: J = serde_json::from_str(text).map_err(|e| Error::value(format!("not JSON: {e}")))?;
    let (rows, weight): (Vec<serde_json::Map<String, J>>, Option<String>) = match doc {
        J::Array(items) => (rows_of(items)?, None),
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
                Some(J::Array(items)) => (rows_of(items)?, weight),
                _ => return Err(Error::value("\"rows\" is a list of rows")),
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
                        .map(|(k, v)| (k.clone(), v[i].clone()))
                        .collect()
                })
                .collect();
            (rows, None)
        }
        _ => {
            return Err(Error::value(
                "the data is a list of rows, {\"rows\": [...]}, or columns",
            ))
        }
    };
    let number = |row: usize, key: &str, v: &J| -> Result<Option<f64>> {
        let x = match v {
            J::Null => return Ok(None),
            J::Number(n) => n.as_f64(),
            J::String(s) if s.trim().is_empty() => return Ok(None),
            J::String(s) => s.trim().parse::<f64>().ok(),
            _ => None,
        };
        match x {
            Some(x) if x.is_finite() => Ok(Some(x)),
            Some(x) if x.is_nan() => Ok(None),
            _ => Err(Error::value(format!(
                "row {row}: {key} is {v}, not a number"
            ))),
        }
    };
    let mut out = Observations {
        weight_column: weight.clone(),
        observed: names.iter().map(|n| (n.clone(), 0)).collect(),
        ..Observations::default()
    };
    let mut weights = Vec::new();
    for (r, row) in rows.iter().enumerate() {
        let mut values = IndexMap::new();
        for name in names {
            if let Some(v) = row.get(name) {
                if let Some(x) = number(r, name, v)? {
                    values.insert(name.clone(), x);
                    out.observed[name] += 1;
                }
            }
        }
        if let Some(w) = &weight {
            let x = row.get(w).map(|v| number(r, w, v)).transpose()?.flatten();
            match x {
                Some(x) if x < 0.0 => {
                    return Err(Error::value(format!("row {r}: its weight {w} is negative")))
                }
                Some(x) => weights.push(x),
                None => weights.push(1.0),
            }
        }
        out.rows.push(values);
    }
    if weight.is_some() {
        out.weights = Some(weights);
    }
    Ok(out)
}

fn rows_of(
    items: Vec<serde_json::Value>,
) -> Result<Vec<serde_json::Map<String, serde_json::Value>>> {
    items
        .into_iter()
        .enumerate()
        .map(|(i, item)| match item {
            serde_json::Value::Object(m) => Ok(m),
            other => Err(Error::value(format!(
                "row {i} is {other}: a row is an object, name: value"
            ))),
        })
        .collect()
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
            let params = start_params(q);
            out.insert(
                name.clone(),
                (log_likelihood_of(q, &params, &data), data.y.len()),
            );
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
                let data = self.data_of(&self.quantities[name], rows, None);
                (name.clone(), (data.y.len(), data.skipped))
            })
            .collect()
    }

    /// The names a row of data may give these rules: the quantities, and
    /// the inputs their parts read.
    pub fn data_names(&self) -> Vec<String> {
        self.order()
            .iter()
            .cloned()
            .chain(self.free_inputs())
            .collect()
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
            if q.parts
                .iter()
                .zip(held)
                .all(|(_, h)| h.contains(&Hold::ALL))
            {
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
            todo.push((q, held, data));
        }
        // The work each quantity is, roughly: its rows, times the
        // distributions each row reads, times the stages of its fit.
        let cost: Vec<f64> = todo
            .iter()
            .map(|(q, _, data)| {
                let stages = if q.parts.iter().any(|p| p.family != Family::Normal) {
                    BARRIER.len()
                } else {
                    1
                };
                (data.y.len() * q.parts.len() * stages) as f64
            })
            .collect();
        let total: f64 = cost.iter().sum::<f64>().max(1.0);
        let mut done = 0.0;
        let mut reports = Vec::new();
        let mut fitted: Vec<(String, Vec<Vec<Vec<f64>>>)> = Vec::new();
        let count = todo.len();
        for (index, (q, held, data)) in todo.into_iter().enumerate() {
            let share = cost[index];
            let mut tell = |within: f64| {
                progress(&Progress {
                    fraction: ((done + share * within.clamp(0.0, 1.0)) / total).min(1.0),
                    quantity: q.name.clone(),
                    index,
                    count,
                })
            };
            tell(0.0);
            let (params, report) = fit_quantity(q, &data, held, options, &mut tell)?;
            done += share;
            reports.push(report);
            fitted.push((q.name.clone(), params));
        }
        let overlay = overlay_text(self, &fitted);
        let rules = self
            .parse_over(
                &overlay,
                "<fitted rules>",
                &crate::rules::Origin::Text,
                &mut crate::rules::Given(IndexMap::new()),
            )
            .map_err(|e| Error::value(format!("the fitted rules are not valid: {e}")))?;
        Ok(Fit {
            rules,
            quantities: reports,
            overlay,
        })
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
        let rest = match (names.get(2), at) {
            (None, None) => return Ok((q.name.clone(), Some(i), Hold::ALL)),
            (Some(param), None) => param.to_string(),
            (param, Some(at)) => format!("{}@{at}", param.unwrap_or(&"")),
        };
        let hold = Hold::parse(&rest, &q.parts[i]).map_err(bad)?;
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

/// The parameters of a family that are estimated (indices into
/// [`Family::params`]): a normal's truncation is held.
fn free_params(family: Family) -> &'static [usize] {
    match family {
        Family::Uniform => &[0, 1],
        Family::Triangular => &[0, 1, 2],
        Family::Normal => &[0, 1],
    }
}

/// The width a distribution of `family` with parameters `p` has: what
/// `min_spread` bounds.
fn spread(family: Family, p: &[f64]) -> f64 {
    match family {
        Family::Uniform => p[1] - p[0],
        Family::Triangular => p[2] - p[0],
        Family::Normal => p[1],
    }
}

/// A part's distribution at a row: its parameters interpolated from the
/// points `corners` names.
fn dist_at(part: &Part, params: &[Vec<f64>], corners: &[(usize, f64)]) -> Dist {
    let mut p = [0.0; 4];
    for &(k, w) in corners {
        for (v, column) in p.iter_mut().zip(params) {
            *v += w * column[k];
        }
    }
    part.family.dist(&p[..params.len()])
}

/// Which part decides the quantity's density at `y`: the one whose
/// distribution function is greatest there (`min`) or least (`max`).
fn deciding(combine: Combine, dists: &[Dist], y: f64) -> usize {
    let mut best = 0;
    if dists.len() == 1 {
        return best;
    }
    for (i, d) in dists.iter().enumerate().skip(1) {
        let (c, b) = (d.cdf(y), dists[best].cdf(y));
        let better = match combine {
            Combine::Min => c > b,
            Combine::Max => c < b,
        };
        if better {
            best = i;
        }
    }
    best
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
            let mass = if a > 0.0 {
                upper(a) - upper(b)
            } else {
                phi(b) - phi(a)
            };
            if !(mass > 0.0) {
                return (f64::NEG_INFINITY, [0.0; 4]);
            }
            let (da, db) = (finite_density(a), finite_density(b));
            let (ta, tb) = (finite_times(a, da), finite_times(b, db));
            let lp = -sd.ln() - 0.5 * z * z - 0.5 * (2.0 * std::f64::consts::PI).ln() - mass.ln();
            let d_mean = z / sd - (da - db) / (sd * mass);
            let d_sd = (z * z - 1.0) / sd - (ta - tb) / (sd * mass);
            (lp, [d_mean, d_sd, 0.0, 0.0])
        }
    }
}

/// The standard normal density at `z`, 0 at an infinite one.
fn finite_density(z: f64) -> f64 {
    if z.is_finite() {
        density(z)
    } else {
        0.0
    }
}

/// `z * density(z)`, 0 at an infinite `z`.
fn finite_times(z: f64, d: f64) -> f64 {
    if z.is_finite() {
        z * d
    } else {
        0.0
    }
}

/// The weighted log-likelihood of `data` under `q` with `params`.
fn log_likelihood_of(q: &Quantity, params: &[Vec<Vec<f64>>], data: &Data) -> f64 {
    let mut total = 0.0;
    for (r, &y) in data.y.iter().enumerate() {
        let dists: Vec<Dist> = q
            .parts
            .iter()
            .zip(params)
            .zip(&data.corners[r])
            .map(|((part, p), c)| dist_at(part, p, c))
            .collect();
        let j = deciding(q.combine, &dists, y);
        total += data.weight[r] * log_pdf_grad(&dists[j], y).0;
    }
    total
}

/// The barrier's weights, stage by stage, down to the likelihood alone
/// (see [`Problem::eval`]).
const BARRIER: [f64; 9] = [1e-1, 1e-2, 1e-3, 1e-4, 1e-5, 1e-6, 1e-7, 1e-8, 0.0];

/// One quantity's fitting problem, in units of `scale` (the spread of its
/// observed values): the variables are, per part, per estimated parameter,
/// the value at each point with data.
struct Problem<'a> {
    q: &'a Quantity,
    data: &'a Data,
    /// The parameters as they start; the variables overwrite theirs.
    base: Vec<Vec<Vec<f64>>>,
    /// (part, parameter, point) of each variable.
    vars: Vec<(usize, usize, usize)>,
    /// Per part, the points with data.
    touched: Vec<Vec<usize>>,
    /// `[part][param][point]`: whether the value is estimated.
    free: Vec<Vec<Vec<bool>>>,
    scale: f64,
    floor: f64,
    total_weight: f64,
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
    fn params(&self, x: &[f64]) -> Vec<Vec<Vec<f64>>> {
        let mut p = self.base.clone();
        for (&(i, k, n), v) in self.vars.iter().zip(x) {
            match var_kind(self.q.parts[i].family, k) {
                Var::Linear => p[i][k][n] = v * self.scale,
                Var::LogSd => p[i][k][n] = v.exp() * self.scale,
                Var::ModeLogit => {}
            }
        }
        // A mode after the range it is a share of.
        for (&(i, k, n), v) in self.vars.iter().zip(x) {
            if var_kind(self.q.parts[i].family, k) == Var::ModeLogit {
                let (low, high) = (p[i][0][n], p[i][2][n]);
                p[i][1][n] = low + sigmoid(*v) * (high - low);
            }
        }
        p
    }

    fn start(&self) -> Vec<f64> {
        self.vars
            .iter()
            .map(|&(i, k, n)| {
                let b = &self.base[i];
                match var_kind(self.q.parts[i].family, k) {
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
                match (var_kind(self.q.parts[i].family, k), self.q.parts[i].family) {
                    // A triangular's mode moves with its range.
                    (Var::Linear, Family::Triangular) if self.free[i][1][n] => {
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

    /// Whether every distribution at a point with data is valid and wide
    /// enough.
    fn valid(&self, params: &[Vec<Vec<f64>>]) -> bool {
        for (i, part) in self.q.parts.iter().enumerate() {
            for &n in &self.touched[i] {
                let p: Vec<f64> = params[i].iter().map(|c| c[n]).collect();
                if p.iter().any(|v| !v.is_finite()) {
                    return false;
                }
                if part.family.dist(&p).check().is_err() {
                    return false;
                }
                // Only a width the fit moves is bounded below.
                let f = &self.free[i];
                let moved = match part.family {
                    Family::Uniform => f[0][n] || f[1][n],
                    Family::Triangular => f[0][n] || f[2][n],
                    Family::Normal => f[1][n],
                };
                if moved && spread(part.family, &p) < self.floor {
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
    /// one scale away from the edge) of the deciding
    /// part, and of every other part the side that bounds the quantity: its
    /// `high` under `min` (the least of the parts' values is at most each
    /// one's), its `low` under `max`. Only a uniform's and a triangular's
    /// range is estimated, so only theirs take part.
    fn eval(&self, x: &[f64], mu: f64) -> Option<(f64, Vec<f64>)> {
        let params = self.params(x);
        if !self.valid(&params) {
            return None;
        }
        // d log-likelihood / d parameter at each point, per part and parameter.
        let mut grad: Vec<Vec<Vec<f64>>> = params
            .iter()
            .map(|p| p.iter().map(|c| vec![0.0; c.len()]).collect())
            .collect();
        let mut total = 0.0;
        let mut dists: Vec<Dist> = Vec::with_capacity(self.q.parts.len());
        for (r, &y) in self.data.y.iter().enumerate() {
            let corners = &self.data.corners[r];
            dists.clear();
            dists.extend(
                self.q
                    .parts
                    .iter()
                    .zip(&params)
                    .zip(corners)
                    .map(|((part, p), c)| dist_at(part, p, c)),
            );
            let j = deciding(self.q.combine, &dists, y);
            let (lp, g) = log_pdf_grad(&dists[j], y);
            if !lp.is_finite() {
                return None;
            }
            let w = self.data.weight[r];
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
                let (lo, hi) = match self.q.parts[i].family {
                    Family::Uniform => (0, 1),
                    Family::Triangular => (0, 2),
                    Family::Normal => continue,
                };
                let (low, high) = d.support();
                let sides = [
                    (lo, y - low, -1.0, i == j || self.q.combine == Combine::Max),
                    (hi, high - y, 1.0, i == j || self.q.combine == Combine::Min),
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

/// The weighted standard deviation of `y` (or, when that is 0 to rounding,
/// the size of its values, or 1): the unit a quantity is fitted in.
fn scale_of(data: &Data) -> f64 {
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
    let sd = var.sqrt();
    let size = data.y.iter().fold(0.0f64, |m, y| m.max(y.abs()));
    // Values all one (to rounding) have no spread to speak of.
    if sd > 1e-9 * size && sd.is_finite() {
        return sd;
    }
    if size > 0.0 {
        size
    } else {
        1.0
    }
}

fn fit_quantity(
    q: &Quantity,
    data: &Data,
    held: &[Vec<Hold>],
    options: &FitOptions,
    progress: &mut dyn FnMut(f64),
) -> Result<(Vec<Vec<Vec<f64>>>, QuantityFit)> {
    let before = log_likelihood_of(q, &start_params(q), data);
    let scale = scale_of(data);
    let floor = options.min_spread * scale;
    let total_weight: f64 = data.weight.iter().sum();
    let mut touched: Vec<Vec<usize>> = vec![Vec::new(); q.parts.len()];
    for corners in &data.corners {
        for (i, c) in corners.iter().enumerate() {
            for &(n, _) in c {
                if !touched[i].contains(&n) {
                    touched[i].push(n);
                }
            }
        }
    }
    for t in &mut touched {
        t.sort_unstable();
    }
    let free: Vec<Vec<Vec<bool>>> = q
        .parts
        .iter()
        .zip(held)
        .map(|(part, holds)| {
            (0..part.family.params().len())
                .map(|k| {
                    (0..part.size())
                        .map(|n| {
                            free_params(part.family).contains(&k)
                                && !holds.iter().any(|h| h.holds(k, n))
                        })
                        .collect()
                })
                .collect()
        })
        .collect();
    let base = feasible_start(q, data, &touched, &free, floor)?;
    let mut vars = Vec::new();
    for (i, part) in q.parts.iter().enumerate() {
        for &k in free_params(part.family) {
            for &n in &touched[i] {
                if free[i][k][n] {
                    vars.push((i, k, n));
                }
            }
        }
    }
    let problem = Problem {
        q,
        data,
        base,
        vars,
        touched: touched.clone(),
        free,
        scale,
        floor,
        total_weight,
    };
    let x0 = problem.start();
    if problem.eval(&x0, BARRIER[0]).is_none() {
        return Err(Error::value(format!(
            "{}: {}: cannot start the fit: a row lies outside the range of a \
             distribution, where its low or high is held (fixed)",
            q.source, q.name
        )));
    }
    // Interior point: the barrier's weight lowered stage by stage, each
    // stage starting where the last ended, then the likelihood alone.
    let (mut x, mut iterations, mut converged) = (x0, 0, true);
    let barrier = problem.vars.iter().any(|&(i, k, _)| {
        !(q.parts[i].family == Family::Normal
            || (q.parts[i].family == Family::Triangular && k == 1))
    });
    let stages: &[f64] = if barrier { &BARRIER } else { &[0.0] };
    let n = stages.len() as f64;
    for (stage, &mu) in stages.iter().enumerate() {
        // Within a stage: how many iterations it takes is not known ahead,
        // so its share fills in as 1 - exp(-iterations / 40).
        let mut tell = |it: usize| progress((stage as f64 + 1.0 - (-(it as f64) / 40.0).exp()) / n);
        let (xs, its, done) = minimize(&problem, x, mu, options, &mut tell);
        progress((stage as f64 + 1.0) / n);
        x = xs;
        iterations += its;
        converged &= done;
    }
    let params = problem.params(&x);
    let after = log_likelihood_of(q, &params, data);
    let report = QuantityFit {
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
            .zip(&touched)
            .enumerate()
            .map(|(i, (p, t))| PartFit {
                name: p.name.clone(),
                points: p.size(),
                points_with_data: t.len(),
                estimated: problem.vars.iter().filter(|v| v.0 == i).count(),
            })
            .collect(),
    };
    Ok((params, report))
}

/// The rules' parameters, moved where needed so every row is possible and
/// every distribution at a point with data at least `floor` wide: a
/// uniform's or a triangular's range widened past the values of the rows
/// near each point (each point's `low` below the least of them, `high` above
/// the greatest -- so the range interpolated at a row, a weighted mean of
/// the points', holds the row's value), a normal's `sd` raised to `floor`.
fn feasible_start(
    q: &Quantity,
    data: &Data,
    touched: &[Vec<usize>],
    free: &[Vec<Vec<bool>>],
    floor: f64,
) -> Result<Vec<Vec<Vec<f64>>>> {
    let mut params = start_params(q);
    for (i, part) in q.parts.iter().enumerate() {
        let size = part.size();
        let mut least = vec![f64::INFINITY; size];
        let mut most = vec![f64::NEG_INFINITY; size];
        for (r, &y) in data.y.iter().enumerate() {
            for &(n, _) in &data.corners[r][i] {
                least[n] = least[n].min(y);
                most[n] = most[n].max(y);
            }
        }
        let p = &mut params[i];
        match part.family {
            Family::Uniform | Family::Triangular => {
                let hi = p.len() - 1;
                for &n in &touched[i] {
                    if free[i][0][n] {
                        p[0][n] = p[0][n].min(least[n] - floor);
                    }
                    if free[i][hi][n] {
                        p[hi][n] = p[hi][n].max(most[n] + floor);
                    }
                }
            }
            Family::Normal => {
                for &n in &touched[i] {
                    if free[i][1][n] {
                        p[1][n] = p[1][n].max(floor);
                    }
                }
                for (r, &y) in data.y.iter().enumerate() {
                    let d = dist_at(part, p, &data.corners[r][i]);
                    let (low, high) = d.support();
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
/// feasible set: `x` minimizing `problem.eval`, the iterations taken, and
/// whether it converged.
fn minimize(
    problem: &Problem,
    mut x: Vec<f64>,
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
    let (mut f, mut g) = problem.eval(&x, mu).expect("the start is feasible");
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

/// The fitted parts as a rule file to merge over `rules`: each fitted
/// part's distribution, every parameter at every point.
fn overlay_text(rules: &RuleSet, fitted: &[(String, Vec<Vec<Vec<f64>>>)]) -> String {
    let mut out = String::from("version: 2\nquantities:");
    if fitted.is_empty() {
        out.push_str(" {}\n");
        return out;
    }
    out.push('\n');
    for (name, params) in fitted {
        let q = &rules.quantities[name];
        out.push_str(&format!("  {name}:\n    parts:\n"));
        for (part, p) in q.parts.iter().zip(params) {
            out.push_str(&format!(
                "      {}:\n        {}:\n",
                part.name,
                part.family.name()
            ));
            let shape: Vec<usize> = part.axes.iter().map(|a| a.points.len()).collect();
            for (k, column) in part.family.params().iter().zip(p) {
                out.push_str(&format!("          {k}: {}\n", nested(column, &shape)));
            }
        }
    }
    out
}

/// `values` (the last axis fastest) as nested flow lists of `shape`.
fn nested(values: &[f64], shape: &[usize]) -> String {
    match shape {
        [] => py_float_repr(values[0]),
        [_] => format!(
            "[{}]",
            values
                .iter()
                .map(|v| py_float_repr(*v))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        [n, rest @ ..] => {
            let stride = values.len() / n;
            format!(
                "[{}]",
                values
                    .chunks(stride)
                    .map(|c| nested(c, rest))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
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
        let again = start
            .parse_over(
                &fit.overlay,
                "o.yaml",
                &crate::rules::Origin::Text,
                &mut crate::rules::Given(IndexMap::new()),
            )
            .unwrap();
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
    fn json_data_comes_as_rows_or_columns_and_may_have_holes() {
        let names: Vec<String> = ["water", "grip"].iter().map(|s| s.to_string()).collect();
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
        assert_eq!(o.weights, Some(vec![0.5, 1.0, 1.0, 1.0]));
        assert_eq!(
            o.observed,
            IndexMap::from([("water".to_string(), 3), ("grip".to_string(), 1)])
        );
        let list = observations_from_json(r#"[{"water": 1, "grip": 0.9}]"#, &names).unwrap();
        assert_eq!(list.rows, vec![row(&[("water", 1.0), ("grip", 0.9)])]);
        assert_eq!(list.weights, None);
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
            for k in free_params(d.family()) {
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
}
