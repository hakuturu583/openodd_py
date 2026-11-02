//! Rules: how a backend derives what a run sets from what it was given.
//!
//! A backend's knobs set what an ODD names -- the rain, the sun. What those do
//! to the rest of a run -- the road's grip, how far drivers see -- is the
//! backend's own model, which it writes as rules: a [`RuleSet`], read from
//! YAML. Each rule is a **conditional distribution**: given some inputs, the
//! distribution of a quantity.
//!
//! ```yaml
//! version: 2
//! quantities:
//!   tire_friction:
//!     parts:
//!       water:
//!         at: {water: [0, 50, 100]}       # the input, and the points written
//!         uniform:                        # the distribution at each point
//!           low:  [0.90, 0.65, 0.45]
//!           high: [1.10, 0.85, 0.65]
//! ```
//!
//! A quantity is one or more **parts**. A part reads the inputs `at` names
//! and gives, at each combination of their points, a distribution's
//! parameters: `uniform` (`low`, `high`), `triangular` (`low`, `mode`,
//! `high`) or `normal` (`mean`, `sd`, truncated to `low`..`high`). Between
//! the points each parameter is interpolated linearly (multilinearly over
//! several inputs), and beyond the outermost points it holds. A parameter
//! may be one number, the same at every point.
//!
//! A run draws a value at a **quantile** `p` (0 to 1): the value the
//! distribution has that share of its mass below ([`Dist::quantile`]). With
//! several parts, `combine` (`min`, the default, or `max`) joins the values
//! the parts give at the same quantile: the parts move together, as one
//! common cause would move them, so a quantity's quantile `p` is the least
//! (or greatest) of its parts' quantiles `p`.
//!
//! A rule set is checked when read: every part well formed, and every
//! distribution valid at every point written (`low` not above `high`, a
//! `mode` between them, a positive `sd`) -- which keeps it valid between
//! them, since each parameter is interpolated linearly. Which inputs exist is
//! the backend's to say ([`RuleSet::check_inputs`]); an input the file gives
//! a `default` may be left out when a quantity is read.

use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};
use crate::sources::GitSource;
use crate::value::py_float_repr;
use crate::yaml::{parse_all, Node, NodeExt};

/// The version of the rule file format this reads.
pub const VERSION: i64 = 2;
/// The deepest a chain of `extends` may go.
pub const MAX_EXTENDS_DEPTH: usize = 32;
/// The most points a part may write: the product of its inputs' points.
pub const MAX_POINTS: usize = 1_000_000;
/// Quantiles a combined quantity's mean is integrated over.
const MEAN_STEPS: usize = 512;

// ---------------------------------------------------------------------------
// Distributions
// ---------------------------------------------------------------------------

/// A distribution's family, and the parameters it is written with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    Uniform,
    Triangular,
    /// A normal distribution truncated to `low`..`high`.
    Normal,
}

impl Family {
    pub const ALL: [Family; 3] = [Family::Uniform, Family::Triangular, Family::Normal];

    pub fn name(self) -> &'static str {
        match self {
            Family::Uniform => "uniform",
            Family::Triangular => "triangular",
            Family::Normal => "normal",
        }
    }

    pub fn from_name(name: &str) -> Option<Family> {
        Family::ALL.into_iter().find(|f| f.name() == name)
    }

    /// The parameters it is written with, in order.
    pub fn params(self) -> &'static [&'static str] {
        match self {
            Family::Uniform => &["low", "high"],
            Family::Triangular => &["low", "mode", "high"],
            Family::Normal => &["mean", "sd", "low", "high"],
        }
    }

    /// The distribution of these parameters (in [`Family::params`]' order).
    pub fn dist(self, p: &[f64]) -> Dist {
        match self {
            Family::Uniform => Dist::Uniform {
                low: p[0],
                high: p[1],
            },
            Family::Triangular => Dist::Triangular {
                low: p[0],
                mode: p[1],
                high: p[2],
            },
            Family::Normal => Dist::Normal {
                mean: p[0],
                sd: p[1],
                low: p[2],
                high: p[3],
            },
        }
    }
}

/// A distribution of one value: what a part gives at its inputs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Dist {
    Uniform {
        low: f64,
        high: f64,
    },
    Triangular {
        low: f64,
        mode: f64,
        high: f64,
    },
    /// Normal, truncated to `low`..`high`.
    Normal {
        mean: f64,
        sd: f64,
        low: f64,
        high: f64,
    },
}

impl Dist {
    pub fn family(&self) -> Family {
        match self {
            Dist::Uniform { .. } => Family::Uniform,
            Dist::Triangular { .. } => Family::Triangular,
            Dist::Normal { .. } => Family::Normal,
        }
    }

    /// Its parameters, in [`Family::params`]' order.
    pub fn params(&self) -> Vec<f64> {
        match *self {
            Dist::Uniform { low, high } => vec![low, high],
            Dist::Triangular { low, mode, high } => vec![low, mode, high],
            Dist::Normal {
                mean,
                sd,
                low,
                high,
            } => vec![mean, sd, low, high],
        }
    }

    /// Why it is no distribution, if it is not.
    pub fn check(&self) -> std::result::Result<(), String> {
        let (low, high) = self.support();
        if low > high {
            return Err(format!(
                "low ({}) must not be above high ({})",
                py_float_repr(low),
                py_float_repr(high)
            ));
        }
        match *self {
            Dist::Triangular { mode, .. } if !(low <= mode && mode <= high) => Err(format!(
                "mode ({}) must be between low and high",
                py_float_repr(mode)
            )),
            Dist::Normal { sd, .. }
                if sd.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater) =>
            {
                Err(format!("sd ({}) must be positive", py_float_repr(sd)))
            }
            Dist::Normal { .. } if low == high => Err(
                "a normal distribution is truncated to low..high, which must not be empty".into(),
            ),
            _ => Ok(()),
        }
    }

    /// The least and greatest value it gives.
    pub fn support(&self) -> (f64, f64) {
        match *self {
            Dist::Uniform { low, high }
            | Dist::Triangular { low, high, .. }
            | Dist::Normal { low, high, .. } => (low, high),
        }
    }

    /// The value with a share `p` (clamped to 0..1) of the mass below it.
    pub fn quantile(&self, p: f64) -> f64 {
        let p = if p.is_nan() { 0.5 } else { p.clamp(0.0, 1.0) };
        match *self {
            Dist::Uniform { low, high } => low + p * (high - low),
            Dist::Triangular { low, mode, high } => {
                let w = high - low;
                if w <= 0.0 {
                    return low;
                }
                let c = (mode - low) / w;
                if p < c {
                    low + (p * w * (mode - low)).sqrt()
                } else {
                    high - ((1.0 - p) * w * (high - mode)).sqrt()
                }
            }
            Dist::Normal {
                mean,
                sd,
                low,
                high,
            } => {
                let (a, b) = ((low - mean) / sd, (high - mean) / sd);
                // Work in the tail the interval sits in, for precision.
                let z = if a > 0.0 {
                    let (qa, qb) = (upper(a), upper(b));
                    -inverse_phi(qa - p * (qa - qb))
                } else {
                    let (pa, pb) = (phi(a), phi(b));
                    inverse_phi(pa + p * (pb - pa))
                };
                (mean + sd * z).clamp(low, high)
            }
        }
    }

    /// The share of its mass at or below `x`.
    pub fn cdf(&self, x: f64) -> f64 {
        let (low, high) = self.support();
        if x < low {
            return 0.0;
        }
        if x >= high {
            return 1.0;
        }
        match *self {
            Dist::Uniform { low, high } => (x - low) / (high - low),
            Dist::Triangular { low, mode, high } => {
                let w = high - low;
                if mode > low && x <= mode {
                    (x - low).powi(2) / (w * (mode - low))
                } else {
                    1.0 - (high - x).powi(2) / (w * (high - mode))
                }
            }
            Dist::Normal {
                mean,
                sd,
                low,
                high,
            } => {
                let (a, b, z) = ((low - mean) / sd, (high - mean) / sd, (x - mean) / sd);
                if a > 0.0 {
                    (upper(a) - upper(z)) / (upper(a) - upper(b))
                } else {
                    (phi(z) - phi(a)) / (phi(b) - phi(a))
                }
            }
        }
        .clamp(0.0, 1.0)
    }

    /// Its density at `x` (infinite at a point mass).
    pub fn pdf(&self, x: f64) -> f64 {
        let (low, high) = self.support();
        if x < low || x > high {
            return 0.0;
        }
        if high <= low {
            return f64::INFINITY;
        }
        match *self {
            Dist::Uniform { low, high } => 1.0 / (high - low),
            Dist::Triangular { low, mode, high } => {
                let w = high - low;
                if x < mode {
                    2.0 * (x - low) / (w * (mode - low))
                } else if x > mode {
                    2.0 * (high - x) / (w * (high - mode))
                } else {
                    2.0 / w
                }
            }
            Dist::Normal {
                mean,
                sd,
                low,
                high,
            } => {
                let (a, b, z) = ((low - mean) / sd, (high - mean) / sd, (x - mean) / sd);
                let mass = if a > 0.0 {
                    upper(a) - upper(b)
                } else {
                    phi(b) - phi(a)
                };
                density(z) / (sd * mass)
            }
        }
    }

    pub fn mean(&self) -> f64 {
        match *self {
            Dist::Uniform { low, high } => (low + high) / 2.0,
            Dist::Triangular { low, mode, high } => (low + mode + high) / 3.0,
            Dist::Normal {
                mean,
                sd,
                low,
                high,
            } => {
                let (a, b) = ((low - mean) / sd, (high - mean) / sd);
                let mass = if a > 0.0 {
                    upper(a) - upper(b)
                } else {
                    phi(b) - phi(a)
                };
                (mean + sd * (density(a) - density(b)) / mass).clamp(low, high)
            }
        }
    }
}

const SQRT_2: f64 = std::f64::consts::SQRT_2;

/// The standard normal's distribution function.
pub(crate) fn phi(z: f64) -> f64 {
    0.5 * libm::erfc(-z / SQRT_2)
}

/// Its upper tail, 1 - phi(z), without cancelling.
pub(crate) fn upper(z: f64) -> f64 {
    0.5 * libm::erfc(z / SQRT_2)
}

pub(crate) fn density(z: f64) -> f64 {
    (-0.5 * z * z).exp() / (2.0 * std::f64::consts::PI).sqrt()
}

/// The standard normal's quantile function: Acklam's approximation, then a
/// Halley step, good to about 1e-15.
fn inverse_phi(p: f64) -> f64 {
    if p <= 0.0 {
        return f64::NEG_INFINITY;
    }
    if p >= 1.0 {
        return f64::INFINITY;
    }
    const A: [f64; 6] = [
        -3.969683028665376e+01,
        2.209460984245205e+02,
        -2.759285104469687e+02,
        1.383_577_518_672_69e2,
        -3.066479806614716e+01,
        2.506628277459239e+00,
    ];
    const B: [f64; 5] = [
        -5.447609879822406e+01,
        1.615858368580409e+02,
        -1.556989798598866e+02,
        6.680131188771972e+01,
        -1.328068155288572e+01,
    ];
    const C: [f64; 6] = [
        -7.784894002430293e-03,
        -3.223964580411365e-01,
        -2.400758277161838e+00,
        -2.549732539343734e+00,
        4.374664141464968e+00,
        2.938163982698783e+00,
    ];
    const D: [f64; 4] = [
        7.784695709041462e-03,
        3.224671290700398e-01,
        2.445134137142996e+00,
        3.754408661907416e+00,
    ];
    let low = 0.02425;
    let x = if p < low {
        let q = (-2.0 * p.ln()).sqrt();
        (((((C[0] * q + C[1]) * q + C[2]) * q + C[3]) * q + C[4]) * q + C[5])
            / ((((D[0] * q + D[1]) * q + D[2]) * q + D[3]) * q + 1.0)
    } else if p <= 1.0 - low {
        let q = p - 0.5;
        let r = q * q;
        (((((A[0] * r + A[1]) * r + A[2]) * r + A[3]) * r + A[4]) * r + A[5]) * q
            / (((((B[0] * r + B[1]) * r + B[2]) * r + B[3]) * r + B[4]) * r + 1.0)
    } else {
        let q = (-2.0 * (1.0 - p).ln()).sqrt();
        -(((((C[0] * q + C[1]) * q + C[2]) * q + C[3]) * q + C[4]) * q + C[5])
            / ((((D[0] * q + D[1]) * q + D[2]) * q + D[3]) * q + 1.0)
    };
    // One Halley step on phi(x) - p.
    let e = phi(x) - p;
    let u = e * (2.0 * std::f64::consts::PI).sqrt() * (x * x / 2.0).exp();
    x - u / (1.0 + x * u / 2.0)
}

// ---------------------------------------------------------------------------
// Parts and quantities
// ---------------------------------------------------------------------------

/// One input of a part, and the points its distributions are written at.
#[derive(Debug, Clone, PartialEq)]
pub struct Axis {
    pub input: String,
    /// Increasing.
    pub points: Vec<f64>,
}

/// A conditional distribution: given its inputs, a distribution's
/// parameters, written at every combination of its axes' points and
/// interpolated between them.
#[derive(Debug, Clone, PartialEq)]
pub struct Part {
    pub name: String,
    pub axes: Vec<Axis>,
    pub family: Family,
    /// Per parameter (in [`Family::params`]' order), its value at every
    /// combination of the axes' points, the last axis fastest.
    pub values: Vec<Vec<f64>>,
    pub text: String,
    /// What a fit to data holds as written (`fixed:`; [`crate::rules_fit`]).
    pub fixed: Vec<Hold>,
}

/// Values of a part a fit to data holds as written: a parameter (`None`:
/// every one) at a combination of points (`None`: every one).
///
/// Written `param`, `param@input=point,...` (the point named by every input's
/// value, one of the points written) or `@input=point,...` (every parameter
/// there); a part's `fixed: true` holds all of it.
#[derive(Debug, Clone, PartialEq)]
pub struct Hold {
    /// An index into [`Family::params`].
    pub param: Option<usize>,
    /// An index into the part's combinations of points (see [`Part::values`]).
    pub point: Option<usize>,
}

impl Hold {
    /// Every value of a part.
    pub const ALL: Hold = Hold {
        param: None,
        point: None,
    };

    /// The hold `spec` names in `part` (see [`Hold`]).
    pub fn parse(spec: &str, part: &Part) -> std::result::Result<Hold, String> {
        let spec = spec.trim();
        let (param, point) = match spec.split_once('@') {
            Some((p, at)) => (p.trim(), Some(at)),
            None => (spec, None),
        };
        let param = if param.is_empty() && point.is_some() {
            None
        } else {
            let names = part.family.params();
            match names.iter().position(|n| *n == param) {
                Some(k) => Some(k),
                None => {
                    return Err(format!(
                        "'{spec}': {} has no parameter '{param}'; it has {}",
                        part.family.name(),
                        names.join(", ")
                    ))
                }
            }
        };
        let point = match point {
            None => None,
            Some(at) => Some(part.point_of(at).map_err(|e| format!("'{spec}': {e}"))?),
        };
        Ok(Hold { param, point })
    }

    /// As a part's `fixed:` writes it (`mean`, `mean@water=50`,
    /// `@water=50`); `None` for [`Hold::ALL`], which is `fixed: true`.
    pub fn spec(&self, part: &Part) -> Option<String> {
        if *self == Hold::ALL {
            return None;
        }
        let param = self
            .param
            .map(|k| part.family.params()[k])
            .unwrap_or_default();
        Some(match self.point {
            None => param.to_string(),
            Some(n) => {
                let strides = part.strides();
                let at: Vec<String> = part
                    .axes
                    .iter()
                    .enumerate()
                    .map(|(d, a)| {
                        let i = n / strides[d] % a.points.len();
                        format!("{}={}", a.input, py_float_repr(a.points[i]))
                    })
                    .collect();
                format!("{param}@{}", at.join(","))
            }
        })
    }

    /// Whether it holds parameter `k` at combination `n`.
    pub fn holds(&self, k: usize, n: usize) -> bool {
        self.param.is_none_or(|p| p == k) && self.point.is_none_or(|q| q == n)
    }
}

impl Part {
    /// The combination of points `at` names (`input=point,...`, every
    /// input once, each value one of its points).
    pub fn point_of(&self, at: &str) -> std::result::Result<usize, String> {
        let mut given: IndexMap<String, f64> = IndexMap::new();
        for pair in at.split(',') {
            let Some((input, v)) = pair.split_once('=') else {
                return Err(format!("'{}' is not input=point", pair.trim()));
            };
            let input = input.trim();
            let v: f64 = v
                .trim()
                .parse()
                .map_err(|_| format!("{input}={}: not a number", v.trim()))?;
            if given.insert(input.to_string(), v).is_some() {
                return Err(format!("{input} is named twice"));
            }
        }
        let strides = self.strides();
        let mut index = 0;
        for (d, axis) in self.axes.iter().enumerate() {
            let Some(v) = given.shift_remove(&axis.input) else {
                return Err(format!(
                    "a point names every input the part reads: {} is missing",
                    axis.input
                ));
            };
            let Some(i) = axis.points.iter().position(|p| *p == v) else {
                return Err(format!(
                    "{}={} is not one of its points ({})",
                    axis.input,
                    py_float_repr(v),
                    axis.points
                        .iter()
                        .map(|p| py_float_repr(*p))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            };
            index += i * strides[d];
        }
        if let Some(extra) = given.keys().next() {
            return Err(format!("the part does not read {extra}"));
        }
        Ok(index)
    }

    /// Whether `fixed` holds parameter `k` at combination `n`.
    pub fn held(&self, k: usize, n: usize) -> bool {
        self.fixed.iter().any(|h| h.holds(k, n))
    }

    /// How many combinations of points it writes.
    pub fn size(&self) -> usize {
        self.axes.iter().map(|a| a.points.len()).product()
    }

    /// Its distribution at `values` (one per axis, in order), each held at
    /// its axis' outermost points.
    pub fn dist(&self, values: &[f64]) -> Dist {
        let mut params = vec![0.0; self.values.len()];
        for (index, w) in self.weights(values) {
            for (p, column) in params.iter_mut().zip(&self.values) {
                *p += w * column[index];
            }
        }
        self.family.dist(&params)
    }

    /// The combinations of points the parameters at `values` (one per axis,
    /// in order) are interpolated from, and the weight of each: every
    /// parameter there is `sum(w * values[index])`. Weights are positive and
    /// add up to 1.
    pub fn weights(&self, values: &[f64]) -> Vec<(usize, f64)> {
        // Each axis: the lower corner's index and the weight of the upper.
        let cells: Vec<(usize, f64)> = self
            .axes
            .iter()
            .zip(values)
            .map(|(axis, v)| segment(&axis.points, *v))
            .collect();
        let strides = self.strides();
        let mut out = Vec::new();
        for corner in 0..(1usize << cells.len()) {
            let mut w = 1.0;
            let mut index = 0;
            for (d, &(i, t)) in cells.iter().enumerate() {
                let up = corner >> d & 1 == 1;
                let n = self.axes[d].points.len();
                let j = if up { (i + 1).min(n - 1) } else { i };
                w *= if up { t } else { 1.0 - t };
                index += j * strides[d];
            }
            if w != 0.0 {
                out.push((index, w));
            }
        }
        out
    }

    /// The distribution written at combination `index` (see [`Part::values`]).
    pub fn dist_at(&self, index: usize) -> Dist {
        let p: Vec<f64> = self.values.iter().map(|c| c[index]).collect();
        self.family.dist(&p)
    }

    fn strides(&self) -> Vec<usize> {
        let mut strides = vec![1; self.axes.len()];
        for d in (0..self.axes.len().saturating_sub(1)).rev() {
            strides[d] = strides[d + 1] * self.axes[d + 1].points.len();
        }
        strides
    }
}

/// The segment of `points` (increasing) that `v` falls in: its lower
/// point's index and how far along it `v` is, held at the ends.
fn segment(points: &[f64], v: f64) -> (usize, f64) {
    let n = points.len();
    if n == 1 || v <= points[0] {
        return (0, 0.0);
    }
    if v >= points[n - 1] {
        return (n - 1, 0.0);
    }
    let i = points.partition_point(|p| *p <= v) - 1;
    (i, (v - points[i]) / (points[i + 1] - points[i]))
}

/// How a quantity's parts are joined.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Combine {
    Min,
    Max,
}

impl Combine {
    pub fn name(self) -> &'static str {
        match self {
            Combine::Min => "min",
            Combine::Max => "max",
        }
    }

    fn join(self, a: f64, b: f64) -> f64 {
        match self {
            Combine::Min => a.min(b),
            Combine::Max => a.max(b),
        }
    }
}

/// An input a rule file describes: the value it reads when none is given.
#[derive(Debug, Clone, PartialEq)]
pub struct InputInfo {
    pub name: String,
    pub default: Option<f64>,
    pub text: String,
}

/// A quantity: parts whose values at one quantile `combine` joins.
#[derive(Debug, Clone, PartialEq)]
pub struct Quantity {
    pub name: String,
    /// Where it was read from, for messages.
    pub source: String,
    pub combine: Combine,
    pub parts: Vec<Part>,
    pub text: String,
}

/// A quantity's distribution at some inputs: its parts', joined.
#[derive(Debug, Clone, PartialEq)]
pub struct Joined {
    pub combine: Combine,
    pub parts: Vec<(String, Dist)>,
}

impl Joined {
    /// The value at quantile `p`: the parts' values at `p`, joined.
    pub fn quantile(&self, p: f64) -> f64 {
        self.parts
            .iter()
            .map(|(_, d)| d.quantile(p))
            .reduce(|a, b| self.combine.join(a, b))
            .expect("a quantity has a part")
    }

    /// The least and greatest value.
    pub fn support(&self) -> (f64, f64) {
        (self.quantile(0.0), self.quantile(1.0))
    }

    /// The share of its mass at or below `x`. The parts move together, so
    /// the least of them is at or below `x` as soon as any one is.
    pub fn cdf(&self, x: f64) -> f64 {
        let each = self.parts.iter().map(|(_, d)| d.cdf(x));
        match self.combine {
            Combine::Min => each.fold(0.0, f64::max),
            Combine::Max => each.fold(1.0, f64::min),
        }
    }

    pub fn mean(&self) -> f64 {
        if let [(_, d)] = self.parts.as_slice() {
            return d.mean();
        }
        (0..MEAN_STEPS)
            .map(|k| self.quantile((k as f64 + 0.5) / MEAN_STEPS as f64))
            .sum::<f64>()
            / MEAN_STEPS as f64
    }

    /// Which part gives the value at quantile `p` (the first, on a tie).
    pub fn deciding(&self, p: f64) -> &str {
        let v = self.quantile(p);
        self.parts
            .iter()
            .find(|(_, d)| d.quantile(p) == v)
            .map(|(n, _)| n.as_str())
            .unwrap_or_default()
    }

    /// `low`, `high`, `median` and `mean`: what [`RuleSet::evaluate`] gives.
    pub fn summary(&self) -> IndexMap<String, f64> {
        let (low, high) = self.support();
        let mut out = IndexMap::new();
        out.insert("low".to_string(), low);
        out.insert("high".to_string(), high);
        out.insert("median".to_string(), self.quantile(0.5));
        out.insert("mean".to_string(), self.mean());
        out
    }
}

impl Quantity {
    /// The inputs its parts read, each once, in the order first read.
    pub fn inputs(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for part in &self.parts {
            for a in &part.axes {
                if !out.contains(&a.input) {
                    out.push(a.input.clone());
                }
            }
        }
        out
    }

    /// The values `part` reads of `inputs`, in its axes' order: each given
    /// one, else its `default`.
    pub fn values_of(
        &self,
        part: &Part,
        inputs: &IndexMap<String, f64>,
        known: &IndexMap<String, InputInfo>,
    ) -> Result<Vec<f64>> {
        let mut values = Vec::with_capacity(part.axes.len());
        for a in &part.axes {
            let given = inputs
                .get(&a.input)
                .copied()
                .or_else(|| known.get(&a.input).and_then(|i| i.default));
            match given {
                Some(v) if v.is_finite() => values.push(v),
                Some(v) => {
                    return Err(Error::value(format!(
                        "{}: {}.{}: {} is {}, not a finite number",
                        self.source,
                        self.name,
                        part.name,
                        a.input,
                        py_float_repr(v)
                    )))
                }
                None => {
                    return Err(Error::value(format!(
                        "{}: {}.{} reads {}, which was not given and has no default",
                        self.source, self.name, part.name, a.input
                    )))
                }
            }
        }
        Ok(values)
    }

    /// Its distribution at `inputs`.
    pub fn joined(
        &self,
        inputs: &IndexMap<String, f64>,
        known: &IndexMap<String, InputInfo>,
    ) -> Result<Joined> {
        let mut parts = Vec::with_capacity(self.parts.len());
        for part in &self.parts {
            let values = self.values_of(part, inputs, known)?;
            parts.push((part.name.clone(), part.dist(&values)));
        }
        Ok(Joined {
            combine: self.combine,
            parts,
        })
    }
}

/// A rule file another one `extends`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Reference {
    /// A path: relative to the file that names it (a local file, a URL, or a
    /// file in a git checkout), or absolute.
    Path(String),
    /// An `https://` URL, and the SHA-256 its content must have, if pinned.
    Url { url: String, sha256: Option<String> },
    /// A file in a git repository at a revision ([`crate::sources`]).
    Git(GitSource),
}

/// Where a rule file was read from: what a relative [`Reference::Path`] in
/// it is relative to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// Given as text: a relative path is relative to the working directory.
    Text,
    File(PathBuf),
    Url(String),
    Git(GitSource),
}

impl Origin {
    /// The origin a reference made in a file from here names.
    pub fn of(&self, reference: &Reference) -> Result<Origin> {
        Ok(match reference {
            Reference::Url { url, .. } => Origin::Url(url.clone()),
            Reference::Git(g) => Origin::Git(g.clone()),
            Reference::Path(p) => match self {
                Origin::Url(base) if !is_absolute_path(p) => Origin::Url(url_join(base, p)?),
                Origin::Git(g) if !is_absolute_path(p) => Origin::Git(GitSource::new(
                    g.url.clone(),
                    g.rev.clone(),
                    git_join(&g.path, p),
                )?),
                Origin::File(f) if !is_absolute_path(p) => {
                    Origin::File(normalize(&f.parent().unwrap_or(Path::new("")).join(p)))
                }
                _ => Origin::File(normalize(Path::new(p))),
            },
        })
    }

    /// The same file's key, however it is named: a local file's canonical
    /// path (links followed), when it exists.
    fn key(&self) -> String {
        match self {
            Origin::File(p) => p
                .canonicalize()
                .map(|c| c.display().to_string())
                .unwrap_or_else(|_| self.label()),
            _ => self.label(),
        }
    }

    /// How it is named in messages.
    pub fn label(&self) -> String {
        match self {
            Origin::Text => "<text>".into(),
            Origin::File(p) => p.display().to_string(),
            Origin::Url(u) => u.clone(),
            Origin::Git(g) => format!("{}@{}:{}", g.url, g.rev, g.path),
        }
    }
}

/// `path` with `.` and `..` worked out without the file system (a leading
/// `..` of a relative path is kept).
fn normalize(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out: Vec<Component> = Vec::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => match out.last() {
                Some(Component::Normal(_)) => {
                    out.pop();
                }
                Some(Component::RootDir) | Some(Component::Prefix(_)) => {}
                _ => out.push(part),
            },
            other => out.push(other),
        }
    }
    out.iter().collect()
}

fn is_absolute_path(p: &str) -> bool {
    Path::new(p).is_absolute() || p.starts_with("https://") || p.starts_with("http://")
}

/// `rel` against the URL `base`, its `.` and `..` worked out.
fn url_join(base: &str, rel: &str) -> Result<String> {
    let Some(rest) = base.strip_prefix("https://") else {
        return Err(Error::value(format!("{base}: only https:// URLs are read")));
    };
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    let mut parts: Vec<&str> = path.split('/').collect();
    parts.pop(); // the file's own name
    for part in rel.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    Ok(format!("https://{host}/{}", parts.join("/")))
}

/// `rel` against the directory of `path`, inside a repository.
fn git_join(path: &str, rel: &str) -> String {
    let mut parts: Vec<&str> = path.split('/').collect();
    parts.pop();
    for part in rel.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    parts.join("/")
}

/// Reads the rule files a rule file `extends`.
pub trait Fetch {
    /// The text of the file at `origin` (never [`Origin::Text`]).
    fn fetch(&mut self, origin: &Origin) -> Result<String>;
}

/// Reads local files and git sources itself, and asks `url` for an
/// `https://` URL (the host's: Python's urllib, the browser's fetch); without
/// one, a URL is an error.
pub struct Files<'a> {
    pub url: Option<&'a mut FetchUrl<'a>>,
}

/// Reads an `https://` URL's text: the host's.
pub type FetchUrl<'a> = dyn FnMut(&str) -> Result<String> + 'a;

impl Fetch for Files<'_> {
    fn fetch(&mut self, origin: &Origin) -> Result<String> {
        match origin {
            Origin::Text => Err(Error::value("a rule file given as text has no file")),
            Origin::File(p) => std::fs::read_to_string(p)
                .map_err(|e| Error::value(format!("cannot read rules {}: {e}", p.display()))),
            Origin::Git(g) => {
                let (path, _) = crate::sources::checkout(g)?;
                std::fs::read_to_string(&path)
                    .map_err(|e| Error::value(format!("cannot read rules {}: {e}", origin.label())))
            }
            Origin::Url(u) => match self.url.as_mut() {
                Some(f) => f(u),
                None => Err(Error::value(format!(
                    "cannot read rules {u}: no way to fetch a URL was given"
                ))),
            },
        }
    }
}

/// Files already read, by [`Origin::label`]: what the browser fetched ahead.
pub struct Given(pub IndexMap<String, String>);

impl Fetch for Given {
    fn fetch(&mut self, origin: &Origin) -> Result<String> {
        self.0
            .get(&origin.label())
            .cloned()
            .ok_or_else(|| Error::value(format!("cannot read rules {}: not given", origin.label())))
    }
}

/// A rule file read, and the SHA-256 of its text: what a run records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRead {
    pub label: String,
    pub sha256: String,
}

/// Quantities, read from a rule file and the files it extends.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RuleSet {
    pub quantities: IndexMap<String, Quantity>,
    /// The inputs the file describes (a `default`, a line saying what it is).
    pub inputs: IndexMap<String, InputInfo>,
    /// Every file read, the files extended before those extending them.
    pub files: Vec<FileRead>,
    /// The rules as one document, every `extends` worked in: what a later
    /// set merges over ([`RuleSet::merged`]).
    doc: Node,
    /// The quantities in an order where each comes after those it reads.
    order: Vec<String>,
}

impl RuleSet {
    /// The rule set of a rule file's `text`; `source` names it in messages.
    /// It may not extend another file ([`RuleSet::parse_with`] reads those).
    pub fn parse(text: &str, source: &str) -> Result<Self> {
        Self::parse_with(text, source, &Origin::Text, &mut Given(IndexMap::new()))
    }

    /// The rule set of the file at `origin` and the files it extends: a
    /// whole set, checked on its own ([`RuleSet::load_over`] reads one that
    /// only changes another).
    pub fn load(origin: &Origin, fetch: &mut dyn Fetch) -> Result<Self> {
        let text = fetch.fetch(origin)?;
        Self::parse_with(&text, &origin.label(), origin, fetch)
    }

    /// The rule set of a rule file's `text`, read from `origin` (what a
    /// relative path it extends is relative to), and the files it extends,
    /// read with `fetch`.
    pub fn parse_with(
        text: &str,
        source: &str,
        origin: &Origin,
        fetch: &mut dyn Fetch,
    ) -> Result<Self> {
        let mut files = Vec::new();
        let doc = compose(text, source, origin, fetch, &mut Vec::new(), &mut files)?;
        let mut set = Self::from_doc(doc, source)?;
        set.files = files;
        Ok(set)
    }

    /// The file at `origin`, and the files it extends, merged over these
    /// rules ([`RuleSet::merged`]): it need only name what it changes, since
    /// it is checked once merged.
    pub fn load_over(&self, origin: &Origin, fetch: &mut dyn Fetch) -> Result<Self> {
        let text = fetch.fetch(origin)?;
        self.parse_over(&text, &origin.label(), origin, fetch)
    }

    /// A rule file's `text` (read from `origin`), and the files it extends,
    /// merged over these rules: [`RuleSet::load_over`] for text in hand.
    pub fn parse_over(
        &self,
        text: &str,
        source: &str,
        origin: &Origin,
        fetch: &mut dyn Fetch,
    ) -> Result<Self> {
        let mut files = Vec::new();
        let over = compose(text, source, origin, fetch, &mut Vec::new(), &mut files)?;
        let touched: Vec<String> = over
            .get("quantities")
            .and_then(Node::as_mapping)
            .map(|m| m.keys().map(NodeExt::py_str).collect())
            .unwrap_or_default();
        let mut doc = self.doc.clone();
        merge_doc(&mut doc, &over);
        let mut set = Self::from_doc(doc, source)?;
        for (name, q) in &mut set.quantities {
            if !touched.contains(name) {
                if let Some(mine) = self.quantities.get(name) {
                    q.source = mine.source.clone();
                }
            }
        }
        set.files = self.files.iter().cloned().chain(files).collect();
        Ok(set)
    }

    /// The references a rule file's `text` extends, each as the origin it
    /// names from `origin`: what a host fetches ahead ([`Given`]).
    pub fn references(text: &str, source: &str, origin: &Origin) -> Result<Vec<Origin>> {
        let doc = document(text, source)?;
        extends_of(&doc, &Where::new(source))?
            .iter()
            .map(|r| origin.of(r))
            .collect()
    }

    fn from_doc(doc: Node, source: &str) -> Result<Self> {
        let at = Where::new(source);
        let map = mapping(&doc, &at, &["version", "inputs", "quantities"])?;
        let mut inputs = IndexMap::new();
        if let Some(raw) = get(map, "inputs") {
            let at = at.key("inputs");
            for (name, entry) in mapping(raw, &at, &[])? {
                let name = node_name(name, &at)?;
                inputs.insert(name.clone(), read_input(&name, entry, &at.key(&name))?);
            }
        }
        let at = at.key("quantities");
        let quantities = required_mapping(map, "quantities", &at)?;
        let mut out = IndexMap::new();
        for (name, raw) in quantities {
            let name = node_name(name, &at)?;
            let quantity = read_quantity(&name, raw, source, &at.key(&name))?;
            out.insert(name, quantity);
        }
        for name in inputs.keys() {
            if out.contains_key(name) {
                return Err(Where::new(source).key("inputs").key(name).error(format!(
                    "'{name}' is a quantity: a part reads a quantity's value, which it gives itself"
                )));
            }
        }
        let order = order_of(&out, &Where::new(source).key("quantities"))?;
        Ok(RuleSet {
            quantities: out,
            inputs,
            files: Vec::new(),
            doc,
            order,
        })
    }

    /// The quantities, each after the quantities its parts read.
    pub fn order(&self) -> &[String] {
        &self.order
    }

    /// The quantities `quantity`'s parts read, each once.
    pub fn upstream(&self, quantity: &str) -> Result<Vec<String>> {
        Ok(self
            .quantity(quantity)?
            .inputs()
            .into_iter()
            .filter(|i| self.quantities.contains_key(i))
            .collect())
    }

    /// The inputs the parts read that are not quantities: what a caller
    /// gives, in the order first read.
    pub fn free_inputs(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for q in self.quantities.values() {
            for i in q.inputs() {
                if !self.quantities.contains_key(&i) && !out.contains(&i) {
                    out.push(i);
                }
            }
        }
        out
    }

    /// `inputs`, and the value of every quantity `quantity` reads, through
    /// any chain: one given in `inputs` is taken as it is (an observed
    /// value); any other is its value at its own quantile in `quantiles`
    /// (the median, 0.5, when not named), at the inputs it reads.
    pub fn resolved(
        &self,
        quantity: &str,
        inputs: &IndexMap<String, f64>,
        quantiles: &IndexMap<String, f64>,
    ) -> Result<IndexMap<String, f64>> {
        let mut values = inputs.clone();
        let mut needed: Vec<String> = Vec::new();
        let mut stack = self.upstream(quantity)?;
        while let Some(q) = stack.pop() {
            if values.contains_key(&q) || needed.contains(&q) {
                continue;
            }
            stack.extend(self.upstream(&q)?);
            needed.push(q);
        }
        for q in &self.order {
            if needed.contains(q) {
                let p = quantiles.get(q).copied().unwrap_or(0.5);
                let v = self.quantity(q)?.joined(&values, &self.inputs)?.quantile(p);
                values.insert(q.clone(), v);
            }
        }
        Ok(values)
    }

    /// Every quantity's value at `inputs`, each at its own quantile in
    /// `quantiles` (0.5 when not named), in [`RuleSet::order`]: a quantity
    /// given in `inputs` keeps the value given.
    pub fn sample(
        &self,
        inputs: &IndexMap<String, f64>,
        quantiles: &IndexMap<String, f64>,
    ) -> Result<IndexMap<String, f64>> {
        let mut values = inputs.clone();
        let mut out = IndexMap::new();
        for q in &self.order {
            let v = match inputs.get(q) {
                Some(v) => *v,
                None => {
                    let p = quantiles.get(q).copied().unwrap_or(0.5);
                    self.quantity(q)?.joined(&values, &self.inputs)?.quantile(p)
                }
            };
            values.insert(q.clone(), v);
            out.insert(q.clone(), v);
        }
        Ok(out)
    }

    /// This rule set with `over`'s rules merged over it: a quantity, a part
    /// and an input `over` names merges over this one's of that name, field
    /// by field (a part's `at` and its distribution replace), and `null`
    /// removes one.
    pub fn merged(&self, over: &RuleSet) -> Result<RuleSet> {
        let source = over
            .files
            .last()
            .map(|f| f.label.clone())
            .unwrap_or_else(|| "<rules>".into());
        let mut doc = self.doc.clone();
        merge_doc(&mut doc, &over.doc);
        let mut set = Self::from_doc(doc, &source)?;
        // Quantities over did not touch keep where they came from.
        for (name, q) in &mut set.quantities {
            if !over.quantities.contains_key(name) {
                if let Some(mine) = self.quantities.get(name) {
                    q.source = mine.source.clone();
                }
            }
        }
        set.files = self.files.iter().chain(&over.files).cloned().collect();
        Ok(set)
    }

    pub fn quantity(&self, name: &str) -> Result<&Quantity> {
        self.quantities.get(name).ok_or_else(|| {
            let mut known: Vec<String> = self.quantities.keys().cloned().collect();
            known.sort();
            Error::value(format!(
                "unknown quantity '{name}'; the rules give {}",
                crate::model::py_list(&known)
            ))
        })
    }

    /// `quantity`'s distribution at `inputs`; an input not given reads its
    /// `default`.
    /// A quantity it reads that is not in `inputs` is at its median
    /// ([`RuleSet::joined_at`] names their quantiles).
    pub fn joined(&self, quantity: &str, inputs: &IndexMap<String, f64>) -> Result<Joined> {
        self.joined_at(quantity, inputs, &IndexMap::new())
    }

    /// `quantity`'s distribution at `inputs`, the quantities it reads at
    /// their quantiles in `quantiles` ([`RuleSet::resolved`]).
    pub fn joined_at(
        &self,
        quantity: &str,
        inputs: &IndexMap<String, f64>,
        quantiles: &IndexMap<String, f64>,
    ) -> Result<Joined> {
        let values = self.resolved(quantity, inputs, quantiles)?;
        self.quantity(quantity)?.joined(&values, &self.inputs)
    }

    /// `low`, `high`, `median` and `mean` of `quantity` at `inputs`
    /// ([`Joined::summary`]).
    pub fn evaluate(
        &self,
        quantity: &str,
        inputs: &IndexMap<String, f64>,
    ) -> Result<IndexMap<String, f64>> {
        Ok(self.joined(quantity, inputs)?.summary())
    }

    /// `quantity`'s value at quantile `p` (0 to 1) at `inputs`.
    pub fn quantile(&self, quantity: &str, inputs: &IndexMap<String, f64>, p: f64) -> Result<f64> {
        Ok(self.joined(quantity, inputs)?.quantile(p))
    }

    /// Refuse an input that is neither one of `allowed` (the backend's)
    /// nor a quantity of these rules.
    pub fn check_inputs(&self, allowed: &[String]) -> Result<()> {
        for q in self.quantities.values() {
            for part in &q.parts {
                for a in &part.axes {
                    if !allowed.contains(&a.input) && !self.quantities.contains_key(&a.input) {
                        return Err(Error::value(format!(
                            "{}: quantities.{}.parts.{}.at: unknown input '{}'; the rules may read {}",
                            q.source,
                            q.name,
                            part.name,
                            a.input,
                            crate::model::py_list(allowed)
                        )));
                    }
                }
            }
        }
        Ok(())
    }
}

/// The quantities in an order where each comes after the quantities its
/// parts read; a quantity that reads itself, through any chain, is refused.
fn order_of(quantities: &IndexMap<String, Quantity>, at: &Where) -> Result<Vec<String>> {
    fn visit(
        name: &str,
        quantities: &IndexMap<String, Quantity>,
        path: &mut Vec<String>,
        done: &mut Vec<String>,
        at: &Where,
    ) -> Result<()> {
        if done.iter().any(|d| d == name) {
            return Ok(());
        }
        if let Some(k) = path.iter().position(|p| p == name) {
            let mut cycle: Vec<String> = path[k..].to_vec();
            cycle.push(name.to_string());
            return Err(at.key(&path[k]).error(format!(
                "the quantities read each other in a cycle: {}",
                cycle.join(" -> ")
            )));
        }
        path.push(name.to_string());
        for i in quantities[name].inputs() {
            if quantities.contains_key(&i) {
                visit(&i, quantities, path, done, at)?;
            }
        }
        path.pop();
        done.push(name.to_string());
        Ok(())
    }
    let mut done = Vec::new();
    for name in quantities.keys() {
        visit(name, quantities, &mut Vec::new(), &mut done, at)?;
    }
    Ok(done)
}

/// One YAML document of `text`, merge keys (`<<`) worked out, its version
/// checked.
fn document(text: &str, source: &str) -> Result<Node> {
    let docs = parse_all(text).map_err(|e| Error::value(format!("{source}: {e}")))?;
    let [doc] = docs.as_slice() else {
        return Err(Error::value(format!(
            "{source}: expected one YAML document, got {}",
            docs.len()
        )));
    };
    let mut doc = doc.clone();
    doc.apply_merge()
        .map_err(|e| Error::value(format!("{source}: {e}")))?;
    let at = Where::new(source);
    let map = mapping(&doc, &at, &["version", "extends", "inputs", "quantities"])?;
    match get(map, "version").and_then(Node::as_i64) {
        Some(VERSION) => Ok(doc),
        Some(1) => Err(at.error(
            "version 1 (fuzzy rules) is no longer read: write each rule base as a part, its rules as \
             the values at its inputs' points (docs/rules.md, \"From version 1\")",
        )),
        _ => Err(at.error(format!(
            "version must be {VERSION}, got {}",
            get(map, "version")
                .map(NodeExt::py_repr)
                .unwrap_or("None".into())
        ))),
    }
}

/// What a document `extends`: a reference, or a list of them.
fn extends_of(doc: &Node, at: &Where) -> Result<Vec<Reference>> {
    let Some(raw) = doc.get("extends").map(untag) else {
        return Ok(Vec::new());
    };
    let at = at.key("extends");
    let items: Vec<&Node> = match raw {
        Node::Sequence(items) => items.iter().collect(),
        other => vec![other],
    };
    let mut out = Vec::new();
    for (k, item) in items.into_iter().enumerate() {
        let at = at.index(k);
        out.push(match untag(item) {
            Node::String(s) if s.starts_with("https://") => Reference::Url {
                url: s.clone(),
                sha256: None,
            },
            Node::String(s) if s.starts_with("http://") => {
                return Err(at.error(format!("{s}: only https:// URLs are read")))
            }
            Node::String(s) if !s.trim().is_empty() => Reference::Path(s.clone()),
            Node::Mapping(_) if item.get("url").is_some() => {
                let map = mapping(item, &at, &["url", "sha256"])?;
                let url = get(map, "url").map(NodeExt::py_str).unwrap_or_default();
                if !url.starts_with("https://") {
                    return Err(at.error(format!("{url}: only https:// URLs are read")));
                }
                let sha256 = get(map, "sha256").map(|n| n.py_str().to_lowercase());
                if let Some(h) = &sha256 {
                    if h.len() != 64 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
                        return Err(at.error("sha256 must be 64 hex digits"));
                    }
                }
                Reference::Url { url, sha256 }
            }
            Node::Mapping(_) => {
                let map = mapping(item, &at, &["git", "rev", "path"])?;
                let field = |k: &str| get(map, k).map(NodeExt::py_str).unwrap_or_default();
                Reference::Git(
                    GitSource::new(field("git"), field("rev"), field("path"))
                        .map_err(|e| at.error(e))?,
                )
            }
            other => {
                return Err(at.error(format!(
                "expected a path, an https:// URL, {{url, sha256}} or {{git, rev, path}}, got {}",
                other.py_repr()
            )))
            }
        });
    }
    Ok(out)
}

/// `text` with every file it extends merged in under it, as one document:
/// the files extended in order, each over the last, then `text` over them.
fn compose(
    text: &str,
    source: &str,
    origin: &Origin,
    fetch: &mut dyn Fetch,
    stack: &mut Vec<String>,
    files: &mut Vec<FileRead>,
) -> Result<Node> {
    let mut doc = document(text, source)?;
    let at = Where::new(source);
    let mut base: Option<Node> = None;
    for reference in extends_of(&doc, &at)? {
        let next = origin
            .of(&reference)
            .map_err(|e| at.key("extends").error(e))?;
        let label = next.label();
        if stack.contains(&next.key()) || next.key() == origin.key() {
            return Err(at.key("extends").error(format!("{label} extends itself")));
        }
        if stack.len() >= MAX_EXTENDS_DEPTH {
            return Err(at.key("extends").error(format!(
                "extends nests deeper than {MAX_EXTENDS_DEPTH} files"
            )));
        }
        let text = fetch.fetch(&next).map_err(|e| at.key("extends").error(e))?;
        if let Reference::Url {
            sha256: Some(want), ..
        } = &reference
        {
            let got = hex::encode(Sha256::digest(text.as_bytes()));
            if &got != want {
                return Err(at
                    .key("extends")
                    .error(format!("{label}: its SHA-256 is {got}, not {want}")));
            }
        }
        stack.push(origin.key());
        let extended = compose(&text, &label, &next, fetch, stack, files)?;
        stack.pop();
        base = Some(match base {
            None => extended,
            Some(mut b) => {
                merge_doc(&mut b, &extended);
                b
            }
        });
    }
    if let Node::Mapping(map) = &mut doc {
        map.remove("extends");
    }
    files.push(FileRead {
        label: source.to_string(),
        sha256: hex::encode(Sha256::digest(text.as_bytes())),
    });
    Ok(match base {
        None => doc,
        Some(mut b) => {
            merge_doc(&mut b, &doc);
            b
        }
    })
}

/// Merge the rule document `over` into `base` (see [`RuleSet::merged`]).
fn merge_doc(base: &mut Node, over: &Node) {
    let (Node::Mapping(b), Node::Mapping(o)) = (base, untag(over)) else {
        return;
    };
    for (k, v) in o {
        match k.py_str().as_str() {
            "quantities" => merge_named(b, k, v, merge_quantity),
            "inputs" => merge_named(b, k, v, merge_fields),
            _ => {
                b.insert(k.clone(), v.clone());
            }
        }
    }
}

/// `over`'s named entries (quantities, parts, inputs) into `base`'s under
/// `key`: each merged by `merge` over the one of its name, a new one added,
/// and `null` removing one.
fn merge_named(
    base: &mut crate::yaml::Mapping,
    key: &Node,
    over: &Node,
    merge: fn(&mut Node, &Node),
) {
    let Node::Mapping(entries) = untag(over) else {
        base.insert(key.clone(), over.clone());
        return;
    };
    if !matches!(base.get(key), Some(Node::Mapping(_))) {
        base.insert(key.clone(), Node::Mapping(crate::yaml::Mapping::new()));
    }
    let Some(Node::Mapping(mine)) = base.get_mut(key) else {
        return;
    };
    for (name, entry) in entries {
        if matches!(untag(entry), Node::Null) {
            mine.remove(name);
            continue;
        }
        match mine.get_mut(name) {
            Some(existing @ Node::Mapping(_)) => merge(existing, entry),
            _ => {
                mine.insert(name.clone(), entry.clone());
            }
        }
    }
}

/// A quantity: field by field, its parts by name.
fn merge_quantity(base: &mut Node, over: &Node) {
    let (Node::Mapping(b), Node::Mapping(o)) = (base, untag(over)) else {
        return;
    };
    for (k, v) in o {
        if k.py_str() == "parts" {
            merge_named(b, k, v, merge_part);
        } else {
            b.insert(k.clone(), v.clone());
        }
    }
}

/// A part: field by field; a distribution it names replaces the one there.
fn merge_part(base: &mut Node, over: &Node) {
    let (Node::Mapping(b), Node::Mapping(o)) = (base, untag(over)) else {
        return;
    };
    let names_family = o.keys().any(|k| Family::from_name(&k.py_str()).is_some());
    if names_family {
        for f in Family::ALL {
            b.remove(f.name());
        }
    }
    for (k, v) in o {
        b.insert(k.clone(), v.clone());
    }
}

/// An input: field by field.
fn merge_fields(base: &mut Node, over: &Node) {
    let (Node::Mapping(b), Node::Mapping(o)) = (base, untag(over)) else {
        return;
    };
    for (k, v) in o {
        b.insert(k.clone(), v.clone());
    }
}

fn read_input(name: &str, node: &Node, at: &Where) -> Result<InputInfo> {
    let map = mapping(node, at, &["default", "text"])?;
    let default = get(map, "default")
        .map(|d| number(d, &at.key("default")))
        .transpose()?;
    Ok(InputInfo {
        name: name.to_string(),
        default,
        text: get(map, "text").map(NodeExt::py_str).unwrap_or_default(),
    })
}

fn read_quantity(name: &str, node: &Node, source: &str, at: &Where) -> Result<Quantity> {
    let map = mapping(node, at, &["combine", "parts", "text"])?;
    let combine = match get(map, "combine").map(NodeExt::py_str).as_deref() {
        None | Some("min") => Combine::Min,
        Some("max") => Combine::Max,
        Some(other) => {
            return Err(at
                .key("combine")
                .error(format!("expected min or max, got '{other}'")))
        }
    };
    let parts_at = at.key("parts");
    let mut parts = Vec::new();
    for (part, raw) in required_mapping(map, "parts", &parts_at)? {
        let part = node_name(part, &parts_at)?;
        parts.push(read_part(&part, raw, &parts_at.key(&part))?);
    }
    if parts.is_empty() {
        return Err(parts_at.error("no parts"));
    }
    Ok(Quantity {
        name: name.to_string(),
        source: source.to_string(),
        combine,
        parts,
        text: get(map, "text").map(NodeExt::py_str).unwrap_or_default(),
    })
}

fn read_part(name: &str, node: &Node, at: &Where) -> Result<Part> {
    let mut known = vec!["at", "text", "fixed"];
    known.extend(Family::ALL.iter().map(|f| f.name()));
    let map = mapping(node, at, &known)?;
    // Its axes: the inputs `at` names, in order, and their points.
    let axes_at = at.key("at");
    let mut axes = Vec::new();
    for (input, raw) in required_mapping(map, "at", &axes_at)? {
        let input = node_name(input, &axes_at)?;
        let here = axes_at.key(&input);
        let points = match untag(raw) {
            Node::Sequence(_) => numbers(raw, &here)?,
            _ => vec![number(raw, &here)?],
        };
        if points.is_empty() {
            return Err(here.error("no points"));
        }
        if let Some(w) = points.windows(2).find(|w| w[0] >= w[1]) {
            return Err(here.error(format!(
                "points must increase: {} is not below {}",
                py_float_repr(w[0]),
                py_float_repr(w[1])
            )));
        }
        axes.push(Axis { input, points });
    }
    let size = axes
        .iter()
        .try_fold(1usize, |acc, a| acc.checked_mul(a.points.len()))
        .filter(|n| *n <= MAX_POINTS)
        .ok_or_else(|| axes_at.error(format!("more than {MAX_POINTS} points")))?;
    // Its distribution: exactly one family, every parameter given.
    let families: Vec<Family> = Family::ALL
        .into_iter()
        .filter(|f| get(map, f.name()).is_some())
        .collect();
    let family = match families.as_slice() {
        [f] => *f,
        [] => return Err(at.error("no distribution: give one of uniform, triangular, normal")),
        _ => {
            return Err(
                at.error("more than one distribution: give one of uniform, triangular, normal")
            )
        }
    };
    let dist_at = at.key(family.name());
    let params = mapping(
        get(map, family.name()).expect("found above"),
        &dist_at,
        family.params(),
    )?;
    let shape: Vec<usize> = axes.iter().map(|a| a.points.len()).collect();
    let mut values = Vec::new();
    for p in family.params() {
        let here = dist_at.key(p);
        let Some(raw) = get(params, p) else {
            return Err(here.error("missing"));
        };
        values.push(read_grid(raw, &shape, size, &here)?);
    }
    let part = Part {
        name: name.to_string(),
        axes,
        family,
        values,
        text: get(map, "text").map(NodeExt::py_str).unwrap_or_default(),
        fixed: Vec::new(),
    };
    let mut part = part;
    if let Some(raw) = get(map, "fixed") {
        let here = at.key("fixed");
        part.fixed = match untag(raw) {
            Node::Bool(true) => vec![Hold::ALL],
            Node::Bool(false) | Node::Null => Vec::new(),
            Node::Sequence(items) => items
                .iter()
                .map(|item| match untag(item) {
                    Node::String(spec) => Hold::parse(spec, &part).map_err(|e| here.error(e)),
                    _ => Err(here.error(
                        "a parameter (mean), a parameter at a point (mean@water=50), or a point (@water=50)",
                    )),
                })
                .collect::<Result<_>>()?,
            _ => {
                return Err(here.error(
                    "true (every value), or a list of a parameter (mean), a parameter at a point (mean@water=50), or a point (@water=50)",
                ))
            }
        };
    }
    for k in 0..size {
        part.dist_at(k)
            .check()
            .map_err(|e| dist_at.error(format!("{} {e}", point_words(&part, k))))?;
    }
    Ok(part)
}

/// A parameter's values: one number for every point, or nested lists, one
/// level per axis.
fn read_grid(node: &Node, shape: &[usize], size: usize, at: &Where) -> Result<Vec<f64>> {
    if let Node::Number(_) = untag(node) {
        return Ok(vec![number(node, at)?; size]);
    }
    fn walk(node: &Node, shape: &[usize], at: &Where, out: &mut Vec<f64>) -> Result<()> {
        let Some((&n, rest)) = shape.split_first() else {
            out.push(number(node, at)?);
            return Ok(());
        };
        let Node::Sequence(items) = untag(node) else {
            return Err(at.error(format!(
                "expected a list of {n} (one per point), got {}",
                node.py_repr()
            )));
        };
        if items.len() != n {
            return Err(at.error(format!(
                "expected {n} values (one per point), got {}",
                items.len()
            )));
        }
        for (k, item) in items.iter().enumerate() {
            walk(item, rest, &at.index(k), out)?;
        }
        Ok(())
    }
    let mut out = Vec::with_capacity(size);
    walk(node, shape, at, &mut out)?;
    Ok(out)
}

/// Where combination `k` of a part's points is, in words: `at water=50:`.
fn point_words(part: &Part, mut k: usize) -> String {
    if part.axes.is_empty() {
        return "everywhere:".into();
    }
    let mut words = Vec::new();
    for a in part.axes.iter().rev() {
        let n = a.points.len();
        words.push(format!("{}={}", a.input, py_float_repr(a.points[k % n])));
        k /= n;
    }
    words.reverse();
    format!("at {}:", words.join(", "))
}

/// Where in a file a node is, for messages: `file: quantities.x.rules[0]`.
#[derive(Clone)]
struct Where {
    source: String,
    path: String,
}

impl Where {
    fn new(source: &str) -> Self {
        Where {
            source: source.to_string(),
            path: String::new(),
        }
    }

    fn key(&self, key: &str) -> Self {
        let path = if self.path.is_empty() {
            key.to_string()
        } else {
            format!("{}.{key}", self.path)
        };
        Where {
            source: self.source.clone(),
            path,
        }
    }

    fn index(&self, i: usize) -> Self {
        Where {
            source: self.source.clone(),
            path: format!("{}[{i}]", self.path),
        }
    }

    fn error(&self, message: impl std::fmt::Display) -> Error {
        if self.path.is_empty() {
            Error::value(format!("{}: {message}", self.source))
        } else {
            Error::value(format!("{}: {}: {message}", self.source, self.path))
        }
    }
}

fn untag(node: &Node) -> &Node {
    match node {
        Node::Tagged(t) => untag(&t.value),
        other => other,
    }
}

fn get<'a>(map: &'a crate::yaml::Mapping, key: &str) -> Option<&'a Node> {
    map.get(key).map(untag)
}

/// `node` as a mapping with only the keys in `known`.
fn mapping<'a>(node: &'a Node, at: &Where, known: &[&str]) -> Result<&'a crate::yaml::Mapping> {
    let Node::Mapping(map) = untag(node) else {
        return Err(at.error(format!("expected a mapping, got {}", node.py_repr())));
    };
    if !known.is_empty() {
        let mut unknown: Vec<String> = map
            .keys()
            .map(NodeExt::py_str)
            .filter(|k| !known.contains(&k.as_str()))
            .collect();
        if !unknown.is_empty() {
            unknown.sort();
            return Err(at.error(format!(
                "unknown fields {}",
                crate::model::py_list(&unknown)
            )));
        }
    }
    Ok(map)
}

fn required_mapping<'a>(
    map: &'a crate::yaml::Mapping,
    key: &str,
    at: &Where,
) -> Result<&'a crate::yaml::Mapping> {
    match get(map, key) {
        Some(node) => mapping(node, at, &[]),
        None => Err(at.error("missing")),
    }
}

fn number(node: &Node, at: &Where) -> Result<f64> {
    let node = untag(node);
    match node {
        Node::Number(n) => match n.as_f64() {
            Some(x) if x.is_finite() => Ok(x),
            _ => Err(at.error(format!("expected a finite number, got {}", node.py_repr()))),
        },
        _ => Err(at.error(format!("expected a number, got {}", node.py_repr()))),
    }
}

fn numbers(node: &Node, at: &Where) -> Result<Vec<f64>> {
    let Node::Sequence(items) = untag(node) else {
        return Err(at.error(format!("expected a list, got {}", node.py_repr())));
    };
    items.iter().map(|n| number(n, at)).collect()
}

fn is_name(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !matches!(s, "AND" | "OR" | "NOT" | "IS")
}

fn node_name(node: &Node, at: &Where) -> Result<String> {
    let s = node.py_str();
    if matches!(untag(node), Node::String(_)) && is_name(&s) {
        Ok(s)
    } else {
        Err(at.error(format!(
            "{} is not a name (letters, digits, _)",
            node.py_repr()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Visibility from fog, and the speed a driver keeps from the
    /// visibility: a chain.
    const CHAIN: &str = r#"version: 2
quantities:
  sight_speed:
    parts:
      sight:
        at: {visibility: [0, 400]}
        uniform: {low: [0, 20], high: [0, 40]}
  visibility:
    parts:
      fog:
        at: {fog: [0, 100]}
        uniform: {low: [200, 0], high: [400, 200]}
"#;

    fn values(pairs: &[(&str, f64)]) -> IndexMap<String, f64> {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    #[test]
    fn a_chain_is_ordered_and_read_through() {
        let set = RuleSet::parse(CHAIN, "chain.yaml").unwrap();
        assert_eq!(set.order(), ["visibility", "sight_speed"]);
        assert_eq!(set.upstream("sight_speed").unwrap(), ["visibility"]);
        assert_eq!(set.free_inputs(), ["fog"]);
        // fog 50: visibility 100-300, median 200; sight_speed at 200 is 10-20.
        let fog = values(&[("fog", 50.0)]);
        assert_eq!(set.evaluate("sight_speed", &fog).unwrap()["low"], 10.0);
        // The visibility at its own quantile, 0: 100 m, so 5-10.
        let q = values(&[("visibility", 0.0)]);
        let j = set.joined_at("sight_speed", &fog, &q).unwrap();
        assert_eq!(j.support(), (5.0, 10.0));
        // A visibility given is taken as observed.
        let seen = values(&[("fog", 50.0), ("visibility", 400.0)]);
        assert_eq!(set.quantile("sight_speed", &seen, 1.0).unwrap(), 40.0);
        let all = set.sample(&fog, &values(&[("sight_speed", 1.0)])).unwrap();
        assert_eq!(all["visibility"], 200.0);
        assert_eq!(all["sight_speed"], 20.0);
        set.check_inputs(&["fog".to_string()]).unwrap();
    }

    #[test]
    fn a_part_says_what_a_fit_holds() {
        let text = r#"version: 2
quantities:
  q:
    parts:
      p:
        at: {a: [0, 10], b: [0, 1]}
        normal: {mean: 0, sd: 1, low: -9, high: 9}
        fixed: [sd, "mean@a=10,b=1", "@b=0,a=0"]
"#;
        let set = RuleSet::parse(text, "f.yaml").unwrap();
        let part = &set.quantities["q"].parts[0];
        let holds = &part.fixed;
        assert_eq!(
            holds[0],
            Hold {
                param: Some(1),
                point: None
            }
        );
        assert_eq!(
            holds[1],
            Hold {
                param: Some(0),
                point: Some(3)
            }
        );
        assert_eq!(
            holds[2],
            Hold {
                param: None,
                point: Some(0)
            }
        );
        assert!(part.held(1, 2) && part.held(0, 3) && part.held(3, 0));
        assert!(!part.held(0, 1));
        let specs: Vec<_> = holds.iter().map(|h| h.spec(part).unwrap()).collect();
        assert_eq!(specs, ["sd", "mean@a=10.0,b=1.0", "@a=0.0,b=0.0"]);
        for h in holds {
            assert_eq!(&Hold::parse(&h.spec(part).unwrap(), part).unwrap(), h);
        }
        let all = text.replace(
            "fixed: [sd, \"mean@a=10,b=1\", \"@b=0,a=0\"]",
            "fixed: true",
        );
        let set = RuleSet::parse(&all, "f.yaml").unwrap();
        assert_eq!(set.quantities["q"].parts[0].fixed, [Hold::ALL]);
        for (bad, says) in [
            (
                "[median]",
                "normal has no parameter 'median'; it has mean, sd, low, high",
            ),
            (
                "[\"mean@a=5,b=0\"]",
                "a=5.0 is not one of its points (0.0, 10.0)",
            ),
            (
                "[\"mean@a=0\"]",
                "a point names every input the part reads: b is missing",
            ),
            ("[\"mean@a=0,b=0,c=1\"]", "the part does not read c"),
            ("[1]", "a parameter (mean)"),
            ("yes please", "true (every value)"),
        ] {
            let t = text.replace("[sd, \"mean@a=10,b=1\", \"@b=0,a=0\"]", bad);
            let err = RuleSet::parse(&t, "f.yaml").unwrap_err().to_string();
            assert!(
                err.contains("quantities.q.parts.p.fixed") && err.contains(says),
                "{bad}: {err}"
            );
        }
    }

    #[test]
    fn a_cycle_is_refused() {
        let text = r#"version: 2
quantities:
  a:
    parts: {p: {at: {b: [0, 1]}, uniform: {low: 0, high: 1}}}
  b:
    parts: {p: {at: {a: [0, 1]}, uniform: {low: 0, high: 1}}}
"#;
        let err = RuleSet::parse(text, "c.yaml").unwrap_err().to_string();
        assert!(err.contains("cycle: a -> b -> a"), "{err}");
        let own = "version: 2\nquantities:\n  a:\n    parts: {p: {at: {a: [0, 1]}, uniform: {low: 0, high: 1}}}\n";
        let err = RuleSet::parse(own, "c.yaml").unwrap_err().to_string();
        assert!(err.contains("cycle: a -> a"), "{err}");
    }

    #[test]
    fn an_input_may_not_name_a_quantity() {
        let text = format!("{CHAIN}inputs: {{visibility: {{default: 1}}}}\n");
        let err = RuleSet::parse(&text, "c.yaml").unwrap_err().to_string();
        assert!(err.contains("'visibility' is a quantity"), "{err}");
    }

    const GRIP: &str = r#"version: 2
quantities:
  grip:
    parts:
      water:
        at: {water: [0, 50, 100]}
        uniform:
          low: [0.90, 0.65, 0.45]
          high: [1.10, 0.85, 0.65]
"#;

    const OVER_WATER: &str = r#"version: 2
extends: base.yaml
quantities:
  grip:
    parts:
      water:
        uniform:
          low: [0.95, 0.60, 0.30]
          high: [1.05, 0.80, 0.50]
"#;

    fn inputs(values: &[(&str, f64)]) -> IndexMap<String, f64> {
        values.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    fn at(rules: &RuleSet, q: &str, values: &[(&str, f64)]) -> Result<(f64, f64)> {
        Ok(rules.joined(q, &inputs(values))?.support())
    }

    fn close(got: (f64, f64), want: (f64, f64)) -> bool {
        (got.0 - want.0).abs() < 1e-9 && (got.1 - want.1).abs() < 1e-9
    }

    #[test]
    fn values_are_interpolated_between_the_points_and_held_beyond() {
        let rules = RuleSet::parse(GRIP, "grip.yaml").unwrap();
        assert!(close(
            at(&rules, "grip", &[("water", 0.0)]).unwrap(),
            (0.90, 1.10)
        ));
        assert!(close(
            at(&rules, "grip", &[("water", 25.0)]).unwrap(),
            (0.775, 0.975)
        ));
        assert!(close(
            at(&rules, "grip", &[("water", 70.0)]).unwrap(),
            (0.57, 0.77)
        ));
        assert!(close(
            at(&rules, "grip", &[("water", -5.0)]).unwrap(),
            (0.90, 1.10)
        ));
        assert!(close(
            at(&rules, "grip", &[("water", 140.0)]).unwrap(),
            (0.45, 0.65)
        ));
        let j = rules.joined("grip", &inputs(&[("water", 70.0)])).unwrap();
        assert!((j.quantile(0.5) - 0.67).abs() < 1e-12);
        assert!((j.quantile(0.25) - 0.62).abs() < 1e-12);
        let s = rules.evaluate("grip", &inputs(&[("water", 70.0)])).unwrap();
        assert_eq!(
            s.keys().collect::<Vec<_>>(),
            ["low", "high", "median", "mean"]
        );
        assert!((s["mean"] - 0.67).abs() < 1e-12);
    }

    #[test]
    fn several_inputs_are_interpolated_multilinearly() {
        let text = "version: 2\nquantities:\n  q:\n    parts:\n      p:\n        at: {a: [0, 10], b: [0, 1]}\n        uniform:\n          low: [[0, 10], [20, 30]]\n          high: 100\n";
        let rules = RuleSet::parse(text, "q.yaml").unwrap();
        let low = |a: f64, b: f64| at(&rules, "q", &[("a", a), ("b", b)]).unwrap().0;
        assert_eq!(low(0.0, 0.0), 0.0);
        assert_eq!(low(0.0, 1.0), 10.0);
        assert_eq!(low(10.0, 0.0), 20.0);
        assert!((low(5.0, 0.5) - 15.0).abs() < 1e-12);
        assert!((low(2.5, 0.0) - 5.0).abs() < 1e-12);
        assert_eq!(at(&rules, "q", &[("a", 5.0), ("b", 0.5)]).unwrap().1, 100.0);
    }

    #[test]
    fn parts_move_together_and_combine_joins_them() {
        let text = "version: 2\nquantities:\n  v:\n    combine: min\n    parts:\n      fog:\n        at: {fog: [0, 100]}\n        uniform: {low: [250, 15], high: [400, 40]}\n      light:\n        at: {sun: [-6, 0]}\n        uniform: {low: [50, 250], high: [150, 400]}\n";
        let rules = RuleSet::parse(text, "v.yaml").unwrap();
        // Clear night: light decides throughout.
        let j = rules
            .joined("v", &inputs(&[("fog", 0.0), ("sun", -20.0)]))
            .unwrap();
        assert_eq!(j.support(), (50.0, 150.0));
        assert_eq!(j.deciding(0.5), "light");
        // The parts cross: fog 70 gives 85.5..148, night 50..150.
        let j = rules
            .joined("v", &inputs(&[("fog", 70.0), ("sun", -20.0)]))
            .unwrap();
        assert!((j.quantile(0.0) - 50.0).abs() < 1e-9);
        assert!((j.quantile(1.0) - 148.0).abs() < 1e-9);
        assert!((j.quantile(0.5) - 100.0).abs() < 1e-9);
        assert_eq!(j.deciding(0.5), "light");
        assert_eq!(j.deciding(1.0), "fog");
        // P(min <= x) is the larger of the parts': the least is there once any is.
        assert!((j.cdf(100.0) - 0.5).abs() < 1e-9);
        let max = RuleSet::parse(&text.replace("combine: min", "combine: max"), "v.yaml").unwrap();
        let j = max
            .joined("v", &inputs(&[("fog", 0.0), ("sun", -20.0)]))
            .unwrap();
        assert_eq!(j.support(), (250.0, 400.0));
    }

    #[test]
    fn the_distributions_quantiles_and_cdfs_agree() {
        let dists = [
            Dist::Uniform {
                low: 0.4,
                high: 0.9,
            },
            Dist::Triangular {
                low: 0.0,
                mode: 2.0,
                high: 10.0,
            },
            Dist::Triangular {
                low: 0.0,
                mode: 0.0,
                high: 1.0,
            },
            Dist::Normal {
                mean: 0.7,
                sd: 0.05,
                low: 0.5,
                high: 0.9,
            },
            Dist::Normal {
                mean: 0.0,
                sd: 1.0,
                low: 6.0,
                high: 9.0,
            },
            Dist::Normal {
                mean: 0.0,
                sd: 1.0,
                low: -9.0,
                high: -6.0,
            },
        ];
        for d in dists {
            d.check().unwrap();
            for k in 0..=20 {
                let p = k as f64 / 20.0;
                let x = d.quantile(p);
                let (lo, hi) = d.support();
                assert!(lo <= x && x <= hi, "{d:?} {p} {x}");
                assert!(
                    (d.cdf(x) - p).abs() < 1e-7,
                    "{d:?} p={p} x={x} cdf={}",
                    d.cdf(x)
                );
            }
        }
        // The standard normal's quantiles.
        let n = Dist::Normal {
            mean: 0.0,
            sd: 1.0,
            low: -40.0,
            high: 40.0,
        };
        assert!((n.quantile(0.975) - 1.959963984540054).abs() < 1e-9);
        assert!((n.quantile(0.5)).abs() < 1e-12);
        assert!((inverse_phi(1e-10) + 6.361340902404056).abs() < 1e-8);
        // A triangle's mean, a truncated normal's.
        assert!(
            (Dist::Triangular {
                low: 0.0,
                mode: 2.0,
                high: 10.0
            }
            .mean()
                - 4.0)
                .abs()
                < 1e-12
        );
        let half = Dist::Normal {
            mean: 0.0,
            sd: 1.0,
            low: 0.0,
            high: 40.0,
        };
        assert!((half.mean() - (2.0 / std::f64::consts::PI).sqrt()).abs() < 1e-12);
    }

    #[test]
    fn a_missing_input_reads_its_default_or_is_refused() {
        let rules = RuleSet::parse(GRIP, "grip.yaml").unwrap();
        let e = at(&rules, "grip", &[]).unwrap_err().to_string();
        assert!(e.contains("reads water, which was not given"), "{e}");
        let with_default = GRIP.replace(
            "quantities:",
            "inputs:\n  water: {default: 100, text: road water}\nquantities:",
        );
        let rules = RuleSet::parse(&with_default, "grip.yaml").unwrap();
        assert!(close(at(&rules, "grip", &[]).unwrap(), (0.45, 0.65)));
        assert_eq!(rules.inputs["water"].text, "road water");
    }

    #[test]
    fn malformed_rule_files_are_refused_saying_where() {
        let cases = [
            (GRIP.replace("low: [0.90, 0.65, 0.45]", "low: [0.90, 0.95, 0.45]"), "uniform: at water=50.0: low (0.95) must not be above high (0.85)"),
            (GRIP.replace("low: [0.90, 0.65, 0.45]", "low: [0.90, 0.65]"), "uniform.low: expected 3 values (one per point), got 2"),
            (GRIP.replace("high: [1.10, 0.85, 0.65]", ""), "uniform.high: missing"),
            (GRIP.replace("[0, 50, 100]", "[0, 50, 50]"), "points must increase: 50.0 is not below 50.0"),
            (GRIP.replace("uniform:", "beta:"), "unknown fields ['beta']"),
            (GRIP.replace("        uniform:\n          low: [0.90, 0.65, 0.45]\n          high: [1.10, 0.85, 0.65]\n", ""), "no distribution"),
            (GRIP.replace("uniform:", "triangular:"), "triangular.mode: missing"),
            (GRIP.replace("    parts:", "    combine: mean\n    parts:"), "expected min or max"),
            (GRIP.replace("version: 2", "version: 3"), "version must be 2"),
            (GRIP.replace("version: 2", "version: 1"), "version 1 (fuzzy rules) is no longer read"),
            (GRIP.replace("water:\n        at", "1st:\n        at"), "is not a name"),
            (GRIP.replace("at: {water: [0, 50, 100]}", "at: {water: []}"), "no points"),
        ];
        for (text, want) in cases {
            let e = RuleSet::parse(&text, "grip.yaml").unwrap_err().to_string();
            assert!(e.contains(want), "want {want:?} in {e}");
            assert!(e.starts_with("grip.yaml: "), "{e}");
        }
        let normal = GRIP.replace(
            "uniform:\n          low: [0.90, 0.65, 0.45]\n          high: [1.10, 0.85, 0.65]",
            "normal: {mean: 0.7, sd: [0.05, 0, 0.05], low: 0.3, high: 1.2}",
        );
        let e = RuleSet::parse(&normal, "grip.yaml")
            .unwrap_err()
            .to_string();
        assert!(
            e.contains("normal: at water=50.0: sd (0.0) must be positive"),
            "{e}"
        );
        let mode = GRIP.replace(
            "uniform:\n          low: [0.90, 0.65, 0.45]\n          high: [1.10, 0.85, 0.65]",
            "triangular: {low: 0.4, mode: [0.5, 0.95, 0.5], high: 0.9}",
        );
        let e = RuleSet::parse(&mode, "grip.yaml").unwrap_err().to_string();
        assert!(
            e.contains("mode (0.95) must be between low and high"),
            "{e}"
        );
    }

    fn files(dir: &Path, entries: &[(&str, &str)]) {
        for (name, text) in entries {
            let path = dir.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
    }

    fn load(path: &Path) -> Result<RuleSet> {
        RuleSet::load(&Origin::File(path.to_path_buf()), &mut Files { url: None })
    }

    #[test]
    fn a_file_extends_another_writing_only_what_it_changes() {
        let dir = tempfile::tempdir().unwrap();
        files(
            dir.path(),
            &[
                ("base.yaml", GRIP),
                (
                    "vehicles/mini.yaml",
                    &OVER_WATER.replace("base.yaml", "../base.yaml"),
                ),
            ],
        );
        let rules = load(&dir.path().join("vehicles/mini.yaml")).unwrap();
        // The points are base.yaml's, the distribution mini.yaml's.
        assert!(close(
            at(&rules, "grip", &[("water", 100.0)]).unwrap(),
            (0.30, 0.50)
        ));
        assert!(close(
            at(&rules, "grip", &[("water", 25.0)]).unwrap(),
            (0.775, 0.925)
        ));
        let labels: Vec<&str> = rules.files.iter().map(|f| f.label.as_str()).collect();
        assert!(
            labels[0].ends_with("base.yaml")
                && !labels[0].contains("..")
                && labels[1].ends_with("mini.yaml"),
            "{labels:?}"
        );
        assert_eq!(
            rules.files[0].sha256,
            hex::encode(Sha256::digest(GRIP.as_bytes()))
        );
    }

    #[test]
    fn extending_merges_parts_by_name_a_distribution_replaces_and_null_removes() {
        let dir = tempfile::tempdir().unwrap();
        let two = GRIP.replace("    parts:\n", "    combine: max\n    parts:\n")
            + "      spare:\n        at: {water: [0, 100]}\n        uniform: {low: 0, high: 2}\n";
        let child = "version: 2\nextends: base.yaml\ninputs:\n  water: {default: 100}\nquantities:\n  grip:\n    combine: min\n    parts:\n      spare: null\n      water:\n        normal: {mean: [1.0, 0.75, 0.55], sd: 0.04, low: [0.9, 0.65, 0.45], high: [1.1, 0.85, 0.65]}\n";
        files(dir.path(), &[("base.yaml", &two), ("child.yaml", child)]);
        let base = load(&dir.path().join("base.yaml")).unwrap();
        assert_eq!(base.quantity("grip").unwrap().parts.len(), 2);
        let rules = load(&dir.path().join("child.yaml")).unwrap();
        let q = rules.quantity("grip").unwrap();
        assert_eq!(q.combine, Combine::Min);
        assert_eq!(q.parts.len(), 1);
        // The points kept, the distribution replaced (uniform's fields gone).
        assert_eq!(q.parts[0].family, Family::Normal);
        assert_eq!(q.parts[0].axes[0].points, [0.0, 50.0, 100.0]);
        let j = rules.joined("grip", &IndexMap::new()).unwrap();
        assert!(close(j.support(), (0.45, 0.65)));
        assert!((j.quantile(0.5) - 0.55).abs() < 1e-9);
    }

    #[test]
    fn a_file_naming_only_its_change_is_read_over_rules_in_hand() {
        let dir = tempfile::tempdir().unwrap();
        files(
            dir.path(),
            &[("run.yaml", &OVER_WATER.replace("extends: base.yaml\n", ""))],
        );
        let origin = Origin::File(dir.path().join("run.yaml"));
        // On its own it is no whole set: water has no points.
        let e = load(&dir.path().join("run.yaml")).unwrap_err().to_string();
        assert!(e.contains("parts.water.at: missing"), "{e}");
        let base = RuleSet::parse(GRIP, "backend.yaml").unwrap();
        let rules = base.load_over(&origin, &mut Files { url: None }).unwrap();
        assert!(close(
            at(&rules, "grip", &[("water", 100.0)]).unwrap(),
            (0.30, 0.50)
        ));
        assert_eq!(rules.files.len(), 2);
        let over = RuleSet::parse(
            &OVER_WATER.replace("extends: base.yaml\n", "").replace(
                "water:\n        uniform",
                "water:\n        at: {water: [0, 50, 100]}\n        uniform",
            ),
            "over.yaml",
        )
        .unwrap();
        let merged = base.merged(&over).unwrap();
        assert_eq!(merged.quantity("grip").unwrap().source, "over.yaml");
    }

    #[test]
    fn a_chain_of_files_and_a_list_apply_in_order_and_a_cycle_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        files(
            dir.path(),
            &[
                ("base.yaml", GRIP),
                ("a.yaml", &OVER_WATER.replace("0.30", "0.31")),
                ("b.yaml", &OVER_WATER.replace("0.30", "0.32")),
                (
                    "both.yaml",
                    "version: 2\nextends: [a.yaml, b.yaml]\nquantities: {}\n",
                ),
                ("c1.yaml", "version: 2\nextends: c2.yaml\nquantities: {}\n"),
                ("c2.yaml", "version: 2\nextends: c1.yaml\nquantities: {}\n"),
            ],
        );
        let rules = load(&dir.path().join("both.yaml")).unwrap();
        assert!(close(
            at(&rules, "grip", &[("water", 100.0)]).unwrap(),
            (0.32, 0.50)
        ));
        let e = load(&dir.path().join("c1.yaml")).unwrap_err().to_string();
        assert!(e.contains("extends itself"), "{e}");
        let e = load(&dir.path().join("nowhere.yaml"))
            .unwrap_err()
            .to_string();
        assert!(e.contains("cannot read rules"), "{e}");
    }

    #[test]
    fn a_cycle_through_another_name_of_the_file_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        files(
            dir.path(),
            &[
                (
                    "a.yaml",
                    "version: 2\nextends: sub/../a.yaml\nquantities: {}\n",
                ),
                ("sub/keep", ""),
                (
                    "deep/b.yaml",
                    "version: 2\nextends: ./../deep/./b.yaml\nquantities: {}\n",
                ),
            ],
        );
        for f in ["a.yaml", "deep/b.yaml"] {
            let e = load(&dir.path().join(f)).unwrap_err().to_string();
            assert!(e.contains("extends itself"), "{f}: {e}");
        }
    }

    #[test]
    fn a_chain_deeper_than_the_limit_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let mut entries = Vec::new();
        for k in 0..=MAX_EXTENDS_DEPTH + 1 {
            entries.push((
                format!("f{k}.yaml"),
                format!("version: 2\nextends: f{}.yaml\nquantities: {{}}\n", k + 1),
            ));
        }
        entries.push((format!("f{}.yaml", MAX_EXTENDS_DEPTH + 2), GRIP.to_string()));
        let refs: Vec<(&str, &str)> = entries
            .iter()
            .map(|(a, b)| (a.as_str(), b.as_str()))
            .collect();
        files(dir.path(), &refs);
        let e = load(&dir.path().join("f0.yaml")).unwrap_err().to_string();
        assert!(e.contains("nests deeper than"), "{e}");
    }

    #[test]
    fn a_url_is_read_by_the_host_and_its_relative_paths_against_it() {
        let mut seen = Vec::new();
        let mut url = |u: &str| -> Result<String> {
            seen.push(u.to_string());
            match u {
                "https://example.com/rules/base.yaml" => Ok(GRIP.to_string()),
                "https://example.com/rules/vehicles/mini.yaml" => {
                    Ok(OVER_WATER.replace("base.yaml", "../base.yaml"))
                }
                _ => Err(Error::value("404")),
            }
        };
        let rules = RuleSet::load(
            &Origin::Url("https://example.com/rules/vehicles/mini.yaml".into()),
            &mut Files {
                url: Some(&mut url),
            },
        )
        .unwrap();
        assert!(close(
            at(&rules, "grip", &[("water", 100.0)]).unwrap(),
            (0.30, 0.50)
        ));
        assert_eq!(seen[1], "https://example.com/rules/base.yaml");
        // Without a way to fetch, a URL is an error; plain http is refused.
        let text = OVER_WATER.replace("base.yaml", "https://example.com/rules/base.yaml");
        let e = RuleSet::parse_with(&text, "x.yaml", &Origin::Text, &mut Files { url: None })
            .unwrap_err()
            .to_string();
        assert!(e.contains("no way to fetch a URL"), "{e}");
        let e = RuleSet::parse(
            &OVER_WATER.replace("base.yaml", "http://example.com/b.yaml"),
            "x.yaml",
        )
        .unwrap_err()
        .to_string();
        assert!(e.contains("only https:// URLs are read"), "{e}");
    }

    #[test]
    fn a_pinned_url_must_have_its_digest() {
        let digest = hex::encode(Sha256::digest(GRIP.as_bytes()));
        let given = |pin: &str| {
            let text = OVER_WATER.replace(
                "extends: base.yaml",
                &format!("extends: {{url: https://example.com/base.yaml, sha256: {pin}}}"),
            );
            let mut map = IndexMap::new();
            map.insert(
                "https://example.com/base.yaml".to_string(),
                GRIP.to_string(),
            );
            RuleSet::parse_with(&text, "x.yaml", &Origin::Text, &mut Given(map))
        };
        assert!(given(&digest).is_ok());
        let e = given(&"0".repeat(64)).unwrap_err().to_string();
        assert!(e.contains(&format!("its SHA-256 is {digest}")), "{e}");
    }

    #[test]
    fn the_references_are_what_a_browser_fetches_ahead() {
        let text = OVER_WATER.replace(
            "extends: base.yaml",
            "extends: [base.yaml, https://x.org/a/b.yaml]",
        );
        let refs = RuleSet::references(
            &text,
            "v.yaml",
            &Origin::Url("https://example.com/r/v.yaml".into()),
        )
        .unwrap();
        assert_eq!(
            refs.iter().map(Origin::label).collect::<Vec<_>>(),
            vec!["https://example.com/r/base.yaml", "https://x.org/a/b.yaml"]
        );
    }

    #[test]
    fn a_git_source_is_read_at_its_revision() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        files(&repo, &[("weather/base.yaml", GRIP)]);
        let git = |args: &[&str]| {
            let ok = std::process::Command::new("git")
                .args([
                    "-c",
                    "user.name=t",
                    "-c",
                    "user.email=t@t",
                    "-c",
                    "commit.gpgsign=false",
                ])
                .args(args)
                .current_dir(&repo)
                .output()
                .unwrap()
                .status
                .success();
            assert!(ok, "git {args:?}");
        };
        git(&["init", "--quiet"]);
        git(&["add", "."]);
        git(&["commit", "--quiet", "-m", "base"]);
        git(&["tag", "v1"]);
        std::env::set_var(crate::sources::CACHE_ENV, dir.path().join("cache"));
        let text = OVER_WATER.replace(
            "extends: base.yaml",
            &format!(
                "extends: {{git: {}, rev: v1, path: weather/base.yaml}}",
                repo.display()
            ),
        );
        let rules =
            RuleSet::parse_with(&text, "x.yaml", &Origin::Text, &mut Files { url: None }).unwrap();
        assert!(close(
            at(&rules, "grip", &[("water", 100.0)]).unwrap(),
            (0.30, 0.50)
        ));
        assert!(rules.files[0].label.ends_with("@v1:weather/base.yaml"));
    }

    #[test]
    fn merge_keys_share_part_of_a_mapping() {
        let text = GRIP.replace(
            "        at: {water: [0, 50, 100]}\n",
            "        <<: &points {at: {water: [0, 50, 100]}}\n",
        );
        let rules = RuleSet::parse(&text, "grip.yaml").unwrap();
        assert!(close(
            at(&rules, "grip", &[("water", 25.0)]).unwrap(),
            (0.775, 0.975)
        ));
    }

    #[test]
    fn the_backend_says_which_inputs_exist() {
        let rules = RuleSet::parse(GRIP, "grip.yaml").unwrap();
        assert!(rules.check_inputs(&["water".into()]).is_ok());
        let e = rules
            .check_inputs(&["rain".into()])
            .unwrap_err()
            .to_string();
        assert!(e.contains("unknown input 'water'"), "{e}");
    }
}
