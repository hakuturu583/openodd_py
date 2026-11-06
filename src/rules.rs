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

/// The newest version of the rule file format this reads.
///
/// Version 3 ties the rules to a taxonomy: each input a part reads stands for
/// one of its concepts, and a quantity -- an effect the rules work out -- for
/// none ([`RuleSet::from_doc`]). Version 2 files are still read; their
/// inputs are unclassified ([`RuleSet::unclassified_inputs`]).
pub const VERSION: i64 = 3;
/// The oldest version read.
pub const OLDEST_VERSION: i64 = 2;
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

    /// The parameters a fit to data estimates (indices into
    /// [`Family::params`]): all but a normal's truncation, the physical
    /// range it is held to.
    pub fn fitted_params(self) -> &'static [usize] {
        match self {
            Family::Uniform => &[0, 1],
            Family::Triangular => &[0, 1, 2],
            Family::Normal => &[0, 1],
        }
    }

    /// The indices of `low` and `high` when a fit estimates them -- the
    /// range every value lies in -- else `None` (a normal's are held).
    pub fn fitted_range(self) -> Option<(usize, usize)> {
        match self {
            Family::Uniform => Some((0, 1)),
            Family::Triangular => Some((0, 2)),
            Family::Normal => None,
        }
    }

    /// How wide the distribution of parameters `p` is: `high - low`, or a
    /// normal's spread as truncated -- its interquartile range over 1.349,
    /// which is `sd` untruncated -- so a mean pushed far outside its range,
    /// which squeezes the distribution against an end, narrows it too.
    pub fn spread(self, p: &[f64]) -> f64 {
        match self {
            Family::Uniform => p[1] - p[0],
            Family::Triangular => p[2] - p[0],
            Family::Normal => {
                let d = self.dist(p);
                (d.quantile(0.75) - d.quantile(0.25)) / 1.348_979_500_392_163_4
            }
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
                } else if mode >= high {
                    high
                } else {
                    // The share above the mode, 1 - c, as a ratio: c may round
                    // to 1 with the mode below high.
                    let rest = (high - mode) / w;
                    let e = ((1.0 - p) / rest).clamp(0.0, 1.0);
                    if e < 0.5 {
                        // Near high: from it down.
                        high - (high - mode) * e.sqrt()
                    } else {
                        // Near the mode: from it up, not `high - sqrt(...)`,
                        // which cancels when the mode is tiny next to high:
                        // 1 - sqrt(1 - d) = d / (1 + sqrt(1 - d)).
                        let d = ((p - c) / rest).clamp(0.0, 1.0);
                        mode + (high - mode) * (d / (1.0 + (1.0 - d).sqrt()))
                    }
                }
                .clamp(low, high)
            }
            Dist::Normal {
                mean,
                sd,
                low,
                high,
            } => Truncated::new(mean, sd, low, high).quantile(p),
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
                // Ratios, not a product's quotient: `w * (mode - low)`
                // underflows where the two are tiny.
                let w = high - low;
                if mode > low && x <= mode {
                    ((x - low) / w) * ((x - low) / (mode - low))
                } else {
                    // From the mode up: c + (1 - c)(1 - r^2), with
                    // 1 - r^2 = u (2 - u), rather than 1 - ..., which cancels.
                    let c = (mode - low) / w;
                    let u = (x - mode) / (high - mode);
                    c + (1.0 - c) * u * (2.0 - u)
                }
            }
            Dist::Normal {
                mean,
                sd,
                low,
                high,
            } => Truncated::new(mean, sd, low, high).cdf(x),
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
                    2.0 / w * ((x - low) / (mode - low))
                } else if x > mode {
                    2.0 / w * ((high - x) / (high - mode))
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
                let s = Truncated::new(mean, sd, low, high);
                (s.ln_pdf((x - mean) / sd) - sd.ln()).exp()
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
                let s = Truncated::new(mean, sd, low, high);
                (mean + sd * (s.density_share(s.a) - s.density_share(s.b))).clamp(low, high)
            }
        }
    }
}

const SQRT_2: f64 = std::f64::consts::SQRT_2;

/// A normal distribution truncated to `low..high`, worked in logarithms:
/// a range far in a tail, where its mass underflows to 0 -- a mean many
/// standard deviations outside it -- still reads. A range in the lower tail
/// is worked as its mirror in the upper; in a tail, a value is placed by its
/// offset from the range's near end, which keeps its digits where `mean + sd
/// * z` would cancel.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Truncated {
    mean: f64,
    sd: f64,
    low: f64,
    high: f64,
    /// The ends, standardized.
    pub a: f64,
    pub b: f64,
    /// Whether the range is worked as its mirror, `-b..-a`.
    flip: bool,
    /// The near end worked from, standardized: above 0 in a tail.
    lo: f64,
    /// The range's width, standardized.
    width: f64,
    /// The log of the standard normal's mass between the ends.
    pub ln_mass: f64,
}

impl Truncated {
    pub fn new(mean: f64, sd: f64, low: f64, high: f64) -> Self {
        let (a, b) = ((low - mean) / sd, (high - mean) / sd);
        let flip = b < 0.0;
        let lo = if flip { -b } else { a };
        let width = (high - low) / sd;
        let ln_mass = if lo > 0.0 {
            ln_upper(lo) + (-ln_upper_from(lo, width).exp_m1()).ln()
        } else {
            (phi(b) - phi(a)).ln()
        };
        Truncated {
            mean,
            sd,
            low,
            high,
            a,
            b,
            flip,
            lo,
            width,
            ln_mass,
        }
    }

    fn in_tail(&self) -> bool {
        self.lo > 0.0
    }

    /// The share of the mass at or below `x`.
    pub fn cdf(&self, x: f64) -> f64 {
        let c = if self.in_tail() {
            let d = if self.flip {
                self.high - x
            } else {
                x - self.low
            } / self.sd;
            let c = ln_upper_from(self.lo, d.clamp(0.0, self.width)).exp_m1()
                / ln_upper_from(self.lo, self.width).exp_m1();
            if self.flip {
                1.0 - c
            } else {
                c
            }
        } else {
            let z = ((x - self.mean) / self.sd).clamp(self.a, self.b);
            (phi(z) - phi(self.a)) / (phi(self.b) - phi(self.a))
        };
        c.clamp(0.0, 1.0)
    }

    /// The value with a share `p` of the mass below it.
    pub fn quantile(&self, p: f64) -> f64 {
        let x = if self.in_tail() {
            let q = if self.flip { 1.0 - p } else { p };
            let t = (q * ln_upper_from(self.lo, self.width).exp_m1()).ln_1p();
            let d = offset_for(self.lo, t).clamp(0.0, self.width);
            if self.flip {
                self.high - self.sd * d
            } else {
                self.low + self.sd * d
            }
        } else {
            let (pa, pb) = (phi(self.a), phi(self.b));
            self.mean + self.sd * inverse_phi(pa + p * (pb - pa))
        };
        x.clamp(self.low, self.high)
    }

    /// The log of the standard density at `z` over the mass.
    pub fn ln_pdf(&self, z: f64) -> f64 {
        ln_density(z) - self.ln_mass
    }

    /// The standard density at `z` over the mass: `density(z) / mass`, 0 at
    /// an infinite `z`.
    pub fn density_share(&self, z: f64) -> f64 {
        (ln_density(z) - self.ln_mass).exp()
    }
}

/// The log of the standard normal's density.
fn ln_density(z: f64) -> f64 {
    -0.5 * z * z - 0.5 * (2.0 * std::f64::consts::PI).ln()
}

/// The log of the standard normal's upper tail, `ln(1 - phi(z))`: from
/// `erfc` while that has the precision, then its asymptotic series.
fn ln_upper(z: f64) -> f64 {
    if z < 35.0 {
        return upper(z).ln();
    }
    if z == f64::INFINITY {
        return f64::NEG_INFINITY;
    }
    ln_density(z) - z.ln() + tail_series(z)
}

/// `ln_upper(lo + d) - ln_upper(lo)` (`d >= 0`): far in the tail, where
/// each is a large number and their difference small, from the series of
/// the difference, which keeps its digits.
fn ln_upper_from(lo: f64, d: f64) -> f64 {
    if d == f64::INFINITY {
        return f64::NEG_INFINITY;
    }
    if lo < 35.0 {
        // A small step: its Taylor series, from the hazard h = density /
        // upper (h' = h (h - z)), rather than a difference that cancels.
        let h = (ln_density(lo) - ln_upper(lo)).exp();
        if h * d < 1e-4 {
            let k = h - lo;
            return -h * d - h * k * d * d / 2.0 - h * (k * k + h * k - 1.0) * d * d * d / 6.0;
        }
        return ln_upper(lo + d) - ln_upper(lo);
    }
    -0.5 * d * (2.0 * lo + d) - (d / lo).ln_1p() + tail_series(lo + d) - tail_series(lo)
}

/// The log of the upper tail's asymptotic series beyond the density over
/// `z`: `ln(1 - 1/z^2 + 3/z^4 - 15/z^6)`.
fn tail_series(z: f64) -> f64 {
    let r = 1.0 / (z * z);
    (1.0 - r + 3.0 * r * r - 15.0 * r * r * r).ln()
}

/// The `d >= 0` with `ln_upper_from(lo, d) = t` (`t <= 0`; `lo > 0`).
fn offset_for(lo: f64, t: f64) -> f64 {
    if t == f64::NEG_INFINITY {
        return f64::INFINITY;
    }
    if t >= 0.0 {
        return 0.0;
    }
    let mut d = if lo >= 35.0 {
        -t / lo
    } else {
        inverse_ln_upper(ln_upper(lo) + t) - lo
    }
    .max(0.0);
    // Newton's method; the slope of ln_upper is -density / upper.
    for _ in 0..60 {
        let z = lo + d;
        let slope = if z >= 35.0 {
            -z / tail_series(z).exp()
        } else {
            -(ln_density(z) - ln_upper(z)).exp()
        };
        let step = (ln_upper_from(lo, d) - t) / slope;
        if !step.is_finite() {
            break;
        }
        d = (d - step).max(0.0);
        if step.abs() <= 1e-15 * d.max(f64::MIN_POSITIVE) {
            break;
        }
    }
    d
}

/// The `z` whose upper tail's log is `l`.
fn inverse_ln_upper(l: f64) -> f64 {
    if l == f64::NEG_INFINITY {
        return f64::INFINITY;
    }
    if l > -600.0 {
        return -inverse_phi(l.exp());
    }
    // Newton's method on ln_upper, whose slope is -density / upper.
    let mut z = (-2.0 * l).sqrt();
    for _ in 0..60 {
        let f = ln_upper(z) - l;
        if f.abs() <= 1e-15 * l.abs() {
            break;
        }
        z += f / (ln_density(z) - ln_upper(z)).exp();
    }
    z
}

/// The standard normal's distribution function.
pub(crate) fn phi(z: f64) -> f64 {
    0.5 * libm::erfc(-z / SQRT_2)
}

/// Its upper tail, 1 - phi(z), without cancelling.
pub(crate) fn upper(z: f64) -> f64 {
    0.5 * libm::erfc(z / SQRT_2)
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
    // One Halley step on phi(x) - p -- but not where exp(x^2 / 2)
    // overflows (p subnormal, x below -37), which would make it NaN.
    let e = phi(x) - p;
    let u = e * (2.0 * std::f64::consts::PI).sqrt() * (x * x / 2.0).exp();
    let refined = x - u / (1.0 + x * u / 2.0);
    if refined.is_finite() {
        refined
    } else {
        x
    }
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
    /// Where its values come from, as the part writes it (`basis:`); its
    /// quantity's or its file's when it writes none ([`RuleSet::basis_of`]).
    pub basis: Option<Basis>,
}

/// Where a distribution's values come from. A rule is not part of an ODD: it
/// is an assumption about how faithfully a run renders the conditions it is
/// given, and an assumption says what it rests on.
#[derive(Debug, Clone, PartialEq)]
pub struct Basis {
    /// `fitted` (estimated from data), `expert` (set by someone who knows)
    /// or `literature` (taken from a published source).
    pub kind: String,
    /// The rest as written -- `data` (`file`, `sha256`, `rows`),
    /// `calibration`, `by`, `date`, `reference`, `text` -- as JSON.
    pub details: serde_json::Value,
}

impl Basis {
    pub const KINDS: [&'static str; 3] = ["fitted", "expert", "literature"];
    /// What a part with no basis anywhere reads as (a version 2 set).
    pub const UNSTATED: &'static str = "unstated";

    /// The basis as one JSON object: `kind` and the details.
    pub fn to_json(&self) -> serde_json::Value {
        let mut out = serde_json::Map::new();
        out.insert("kind".into(), self.kind.clone().into());
        if let serde_json::Value::Object(d) = &self.details {
            out.extend(d.clone());
        }
        serde_json::Value::Object(out)
    }
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

    /// The hold `spec` names in a part of `family` over `axes` (see [`Hold`]).
    pub fn parse(spec: &str, family: Family, axes: &[Axis]) -> std::result::Result<Hold, String> {
        let spec = spec.trim();
        let (param, at) = match spec.split_once('@') {
            Some((p, at)) => (p.trim(), Some(at)),
            None => (spec, None),
        };
        let param = (!param.is_empty() || at.is_none()).then_some(param);
        Hold::new(param, at, family, axes).map_err(|e| format!("'{spec}': {e}"))
    }

    /// The hold of parameter `param` (`None`: every one) at the point `at`
    /// names (`input=point,...`; `None`: every point), in a part of `family`
    /// over `axes`.
    pub fn new(
        param: Option<&str>,
        at: Option<&str>,
        family: Family,
        axes: &[Axis],
    ) -> std::result::Result<Hold, String> {
        let param = match param {
            None => None,
            Some(name) => {
                let names = family.params();
                Some(names.iter().position(|n| *n == name).ok_or_else(|| {
                    format!(
                        "{} has no parameter '{name}'; it has {}",
                        family.name(),
                        names.join(", ")
                    )
                })?)
            }
        };
        let point = at.map(|at| point_of(axes, at)).transpose()?;
        Ok(Hold { param, point })
    }

    /// As a part's `fixed:` writes it (`mean`, `mean@water=50`,
    /// `@water=50`); `None` for [`Hold::ALL`], which is `fixed: true`.
    pub fn spec(&self, family: Family, axes: &[Axis]) -> Option<String> {
        if *self == Hold::ALL {
            return None;
        }
        let param = self.param.map(|k| family.params()[k]).unwrap_or_default();
        Some(match self.point {
            None => param.to_string(),
            Some(n) => format!("{param}@{}", point_coords(axes, n).join(",")),
        })
    }

    /// Whether it holds parameter `k` at combination `n`.
    pub fn holds(&self, k: usize, n: usize) -> bool {
        self.param.is_none_or(|p| p == k) && self.point.is_none_or(|q| q == n)
    }
}

/// Per axis, how far apart its points' combinations are (the last axis
/// fastest; see [`Part::values`]).
fn strides_of(axes: &[Axis]) -> Vec<usize> {
    let mut strides = vec![1; axes.len()];
    for d in (0..axes.len().saturating_sub(1)).rev() {
        strides[d] = strides[d + 1] * axes[d + 1].points.len();
    }
    strides
}

/// Combination `k` of `axes`' points, as `input=point` per axis.
pub fn point_coords(axes: &[Axis], k: usize) -> Vec<String> {
    let strides = strides_of(axes);
    axes.iter()
        .enumerate()
        .map(|(d, a)| {
            let i = k / strides[d] % a.points.len();
            format!("{}={}", a.input, py_float_repr(a.points[i]))
        })
        .collect()
}

/// The combination of `axes`' points `at` names (`input=point,...`, every
/// input once, each value one of its points).
pub fn point_of(axes: &[Axis], at: &str) -> std::result::Result<usize, String> {
    let mut given: IndexMap<String, f64> = IndexMap::new();
    for pair in at.split(',') {
        let Some((input, v)) = pair.split_once('=') else {
            return Err(format!("'{}' is not input=point", pair.trim()));
        };
        let input = input.trim();
        let v = crate::value::parse_float(v.trim())
            .ok_or_else(|| format!("{input}={}: not a number", v.trim()))?;
        if given.insert(input.to_string(), v).is_some() {
            return Err(format!("{input} is named twice"));
        }
    }
    let strides = strides_of(axes);
    let mut index = 0;
    for (d, axis) in axes.iter().enumerate() {
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

impl Part {
    /// How many combinations of points it writes.
    pub fn size(&self) -> usize {
        self.axes.iter().map(|a| a.points.len()).product()
    }

    /// Its distribution at `values` (one per axis, in order), each held at
    /// its axis' outermost points.
    pub fn dist(&self, values: &[f64]) -> Dist {
        let mut params = [0.0; 4];
        self.each_corner(values, |index, w| {
            for (p, column) in params.iter_mut().zip(&self.values) {
                *p += w * column[index];
            }
        });
        self.family.dist(&params[..self.values.len()])
    }

    /// The distribution of parameters `params` (per parameter, a value at
    /// every combination of points, as [`Part::values`]) where `corners`
    /// ([`Part::weights`]) says: [`Part::dist`] over other values.
    pub fn dist_with(&self, params: &[Vec<f64>], corners: &[(usize, f64)]) -> Dist {
        let mut p = [0.0; 4];
        for &(index, w) in corners {
            for (v, column) in p.iter_mut().zip(params) {
                *v += w * column[index];
            }
        }
        self.family.dist(&p[..params.len()])
    }

    /// The combinations of points the parameters at `values` (one per axis,
    /// in order) are interpolated from, and the weight of each: every
    /// parameter there is `sum(w * values[index])`. Weights are positive and
    /// add up to 1.
    pub fn weights(&self, values: &[f64]) -> Vec<(usize, f64)> {
        let mut out = Vec::with_capacity(1 << self.axes.len());
        self.each_corner(values, |index, w| out.push((index, w)));
        out
    }

    /// [`Part::weights`], each given to `f` rather than collected.
    fn each_corner(&self, values: &[f64], mut f: impl FnMut(usize, f64)) {
        // Each axis: the lower corner's index and the weight of the upper.
        let cells: Vec<(usize, f64)> = self
            .axes
            .iter()
            .zip(values)
            .map(|(axis, v)| segment(&axis.points, *v))
            .collect();
        let strides = self.strides();
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
                f(index, w);
            }
        }
    }

    /// The distribution written at combination `index` (see [`Part::values`]).
    pub fn dist_at(&self, index: usize) -> Dist {
        let p: Vec<f64> = self.values.iter().map(|c| c[index]).collect();
        self.family.dist(&p)
    }

    fn strides(&self) -> Vec<usize> {
        strides_of(&self.axes)
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

    /// Which of the parts whose distribution functions at a value are `cdfs`
    /// decides the quantity there, and the quantity's distribution function
    /// there: the greatest (`min`: the least of the parts is at or below the
    /// value as soon as any one is) or the least (`max`). The first, on a tie.
    pub fn deciding(self, cdfs: impl IntoIterator<Item = f64>) -> (usize, f64) {
        let mut best = (0, f64::NAN);
        for (i, c) in cdfs.into_iter().enumerate() {
            let better = match self {
                Combine::Min => c > best.1,
                Combine::Max => c < best.1,
            };
            if i == 0 || better {
                best = (i, c);
            }
        }
        best
    }

    fn join(self, a: f64, b: f64) -> f64 {
        match self {
            Combine::Min => a.min(b),
            Combine::Max => a.max(b),
        }
    }
}

/// An input a rule file describes: the value it reads when none is given,
/// and, from version 3, the taxonomy concept it stands for.
#[derive(Debug, Clone, PartialEq)]
pub struct InputInfo {
    pub name: String,
    pub default: Option<f64>,
    pub text: String,
    /// The taxonomy concept whose value the input is (version 3).
    pub concept: Option<String>,
    /// The unit its points and its default are written in (`""`: the
    /// concept's own).
    pub unit: String,
}

/// The taxonomy a version 3 rule file names: the concepts its inputs may be.
#[derive(Debug, Clone, PartialEq)]
pub struct Taxonomy {
    /// Where it was read from.
    pub label: String,
    pub sha256: String,
    /// Each concept the taxonomy declares, and what it is.
    pub concepts: IndexMap<String, crate::model::ConceptKind>,
}

impl Taxonomy {
    /// The taxonomy of an OpenODD file's `text`.
    pub fn parse(text: &str, label: &str) -> Result<Self> {
        use crate::loader::{load_openodd, AnyProbe, Options, Source};
        // One file: its IMPORTs would be read from the process's working
        // directory, not from where the taxonomy was, and its SHA-256 would
        // not cover them.
        let docs = parse_all(text).map_err(|e| Error::value(format!("{label}: {e}")))?;
        if docs.iter().any(|d| d.get("IMPORT").is_some()) {
            return Err(Error::value(format!(
                "{label}: a rule file's taxonomy is one OpenODD file, and this one IMPORTs \
                 others; name a file that holds the whole taxonomy"
            )));
        }
        let loaded = load_openodd(&[Source::Text(text.into())], &Options::default(), &AnyProbe)
            .map_err(|e| Error::value(format!("{label}: {e}")))?;
        Ok(Taxonomy {
            label: label.to_string(),
            sha256: hex::encode(Sha256::digest(text.as_bytes())),
            concepts: loaded
                .odd
                .attributes
                .iter()
                .map(|a| (a.name.clone(), a.concept.clone()))
                .collect(),
        })
    }
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
    /// The basis of the parts that write none.
    pub basis: Option<Basis>,
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
        self.combine
            .deciding(self.parts.iter().map(|(_, d)| d.cdf(x)))
            .1
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
    /// The format's version: the newest of the files read.
    pub version: i64,
    /// The taxonomy its inputs' concepts are in (version 3).
    pub taxonomy: Option<Taxonomy>,
    /// The basis of the parts that, with their quantities, write none.
    pub basis: Option<Basis>,
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
        let mut taxonomy = None;
        let doc = compose(
            text,
            source,
            origin,
            fetch,
            &mut Vec::new(),
            &mut files,
            &mut taxonomy,
        )?;
        let mut set = Self::from_doc(doc, source, taxonomy)?;
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
        let mut taxonomy = None;
        let over = compose(
            text,
            source,
            origin,
            fetch,
            &mut Vec::new(),
            &mut files,
            &mut taxonomy,
        )?;
        let touched: Vec<String> = over
            .get("quantities")
            .and_then(Node::as_mapping)
            .map(|m| m.keys().map(NodeExt::py_str).collect())
            .unwrap_or_default();
        let mut doc = self.doc.clone();
        merge_doc(&mut doc, &over);
        let mut set = Self::from_doc(doc, source, taxonomy.or_else(|| self.taxonomy.clone()))?;
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

    /// The files a rule file's `text` reads, each as the origin it names
    /// from `origin`: what a host fetches ahead ([`Given`]) -- the rule
    /// files it extends ([`RuleSet::extends_references`]), then its taxonomy
    /// ([`RuleSet::taxonomy_reference`]).
    pub fn references(text: &str, source: &str, origin: &Origin) -> Result<Vec<Origin>> {
        let mut refs = Self::extends_references(text, source, origin)?;
        refs.extend(Self::taxonomy_reference(text, source, origin)?);
        Ok(refs)
    }

    /// The rule files a rule file's `text` extends: each a rule file in
    /// turn, whose own references a host follows.
    pub fn extends_references(text: &str, source: &str, origin: &Origin) -> Result<Vec<Origin>> {
        let doc = document(text, source)?;
        extends_of(&doc, &Where::new(source))?
            .iter()
            .map(|r| origin.of(r))
            .collect()
    }

    /// The taxonomy a rule file's `text` names: an OpenODD file, not a rule
    /// file, so a host fetches it and goes no further.
    pub fn taxonomy_reference(text: &str, source: &str, origin: &Origin) -> Result<Option<Origin>> {
        let doc = document(text, source)?;
        taxonomy_of(&doc, &Where::new(source))?
            .map(|r| origin.of(&r))
            .transpose()
    }

    fn from_doc(doc: Node, source: &str, taxonomy: Option<Taxonomy>) -> Result<Self> {
        let at = Where::new(source);
        let map = mapping(&doc, &at, &["version", "basis", "inputs", "quantities"])?;
        let basis = get(map, "basis")
            .map(|b| read_basis(b, &at.key("basis")))
            .transpose()?;
        let version = get(map, "version")
            .and_then(Node::as_i64)
            .unwrap_or(OLDEST_VERSION);
        let mut inputs = IndexMap::new();
        if let Some(raw) = get(map, "inputs") {
            let at = at.key("inputs");
            for (name, entry) in mapping(raw, &at, &[])? {
                let name = node_name(name, &at)?;
                inputs.insert(
                    name.clone(),
                    read_input(&name, entry, version, &at.key(&name))?,
                );
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
        let set = RuleSet {
            quantities: out,
            inputs,
            files: Vec::new(),
            version,
            taxonomy,
            basis,
            doc,
            order,
        };
        if version >= 3 {
            set.check_concepts(source)?;
            set.check_bases(source)?;
        }
        Ok(set)
    }

    /// Version 3: every part says what its values rest on.
    fn check_bases(&self, source: &str) -> Result<()> {
        for q in self.quantities.values() {
            for part in &q.parts {
                if self.basis_of(&q.name, &part.name).is_none() {
                    return Err(Where::new(source)
                        .key("quantities")
                        .key(&q.name)
                        .key("parts")
                        .key(&part.name)
                        .error(format!(
                            "no basis: say what its values rest on -- basis: {{kind: {}}} -- on \
                             the part, its quantity or the file",
                            Basis::KINDS.join(" | ")
                        )));
                }
            }
        }
        Ok(())
    }

    /// The basis of a part: its own, else its quantity's, else the file's;
    /// `None` when none says (a version 2 set's parts may be unstated).
    pub fn basis_of(&self, quantity: &str, part: &str) -> Option<&Basis> {
        let q = self.quantities.get(quantity)?;
        q.parts
            .iter()
            .find(|p| p.name == part)?
            .basis
            .as_ref()
            .or(q.basis.as_ref())
            .or(self.basis.as_ref())
    }

    /// Version 3's roles: every input a part reads stands for a concept of
    /// the taxonomy -- a cause the rules are given -- and the quantities are
    /// the effects they work out.
    fn check_concepts(&self, source: &str) -> Result<()> {
        let at = Where::new(source);
        let Some(taxonomy) = &self.taxonomy else {
            return Err(at.error(
                "version 3 names the taxonomy its inputs are concepts of: taxonomy: <path, URL or git source>",
            ));
        };
        for name in self.free_inputs() {
            let concept = self.inputs.get(&name).and_then(|i| i.concept.as_deref());
            let Some(concept) = concept else {
                return Err(at.key("inputs").key(&name).error(format!(
                    "'{name}' is read by a part but stands for no taxonomy concept: give it \
                     `concept:` (and add the concept to the taxonomy if it has none), or make it \
                     a quantity the rules work out"
                )));
            };
            let at = at.key("inputs").key(&name).key("concept");
            let Some(kind) = taxonomy.concepts.get(concept) else {
                return Err(at.error(format!(
                    "'{concept}' is not a concept of the taxonomy {}",
                    taxonomy.label
                )));
            };
            let crate::model::ConceptKind::Number { unit_type, .. } = kind else {
                return Err(at.error(format!(
                    "'{concept}' is not a number in {}: a part's points are numbers, so an input \
                     stands for a numeric concept",
                    taxonomy.label
                )));
            };
            let unit = &self.inputs[&name].unit;
            if !unit.is_empty() {
                crate::units::Units::new()
                    .check_type(unit, unit_type)
                    .map_err(|e| {
                        Where::new(source)
                            .key("inputs")
                            .key(&name)
                            .key("unit")
                            .error(e.to_string())
                    })?;
            }
        }
        Ok(())
    }

    /// The inputs the parts read that stand for no taxonomy concept: every
    /// free input of a version 2 file, none of a version 3 one (it refuses
    /// them).
    pub fn unclassified_inputs(&self) -> Vec<String> {
        if self.version >= 3 {
            Vec::new()
        } else {
            self.free_inputs()
        }
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
        let taxonomy = over.taxonomy.clone().or_else(|| self.taxonomy.clone());
        let mut set = Self::from_doc(doc, &source, taxonomy)?;
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
    let map = mapping(
        &doc,
        &at,
        &[
            "version",
            "extends",
            "taxonomy",
            "basis",
            "inputs",
            "quantities",
        ],
    )?;
    match get(map, "version").and_then(Node::as_i64) {
        Some(2) if get(map, "taxonomy").is_some() => Err(at.key("taxonomy").error(
            "a taxonomy is named from version 3, which ties each input to one of its concepts",
        )),
        Some(OLDEST_VERSION..=VERSION) => Ok(doc),
        Some(1) => Err(at.error(
            "version 1 (fuzzy rules) is no longer read: write each rule base as a part, its rules as \
             the values at its inputs' points (docs/rules.md, \"From version 1\")",
        )),
        _ => Err(at.error(format!(
            "version must be {OLDEST_VERSION} to {VERSION}, got {}",
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
        out.push(reference_of(item, &at.index(k))?);
    }
    Ok(out)
}

/// The taxonomy a document names, if it names one.
fn taxonomy_of(doc: &Node, at: &Where) -> Result<Option<Reference>> {
    doc.get("taxonomy")
        .map(|raw| reference_of(raw, &at.key("taxonomy")))
        .transpose()
}

/// One reference to another file: a path, an `https://` URL, `{url,
/// sha256}` or `{git, rev, path}`.
fn reference_of(item: &Node, at: &Where) -> Result<Reference> {
    {
        let at = at.clone();
        Ok(match untag(item) {
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
        })
    }
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
    taxonomy: &mut Option<Taxonomy>,
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
        let extended = compose(&text, &label, &next, fetch, stack, files, taxonomy)?;
        stack.pop();
        base = Some(match base {
            None => extended,
            Some(mut b) => {
                merge_doc(&mut b, &extended);
                b
            }
        });
    }
    // The taxonomy this file names, read from where it says, over any a file
    // it extends named.
    if let Some(reference) = taxonomy_of(&doc, &at)? {
        let at = at.key("taxonomy");
        let next = origin.of(&reference).map_err(|e| at.error(e))?;
        let label = next.label();
        let text = fetch.fetch(&next).map_err(|e| at.error(e))?;
        if let Reference::Url {
            sha256: Some(want), ..
        } = &reference
        {
            let got = hex::encode(Sha256::digest(text.as_bytes()));
            if &got != want {
                return Err(at.error(format!("{label}: its SHA-256 is {got}, not {want}")));
            }
        }
        *taxonomy = Some(Taxonomy::parse(&text, &label)?);
    }
    if let Node::Mapping(map) = &mut doc {
        map.remove("extends");
        map.remove("taxonomy");
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
            // A set is of the newest version among its files: a version 2
            // overlay (a fit's) keeps a version 3 set's roles.
            "version" => {
                let newest = match (b.get(k).and_then(Node::as_i64), v.as_i64()) {
                    (Some(a), Some(c)) if a > c => b.get(k).cloned(),
                    _ => Some(v.clone()),
                };
                if let Some(n) = newest {
                    b.insert(k.clone(), n);
                }
            }
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

fn read_input(name: &str, node: &Node, version: i64, at: &Where) -> Result<InputInfo> {
    let known: &[&str] = if version >= 3 {
        &["default", "text", "concept", "unit"]
    } else {
        &["default", "text"]
    };
    let map = mapping(node, at, known)?;
    let default = get(map, "default")
        .map(|d| number(d, &at.key("default")))
        .transpose()?;
    Ok(InputInfo {
        name: name.to_string(),
        default,
        text: get(map, "text").map(NodeExt::py_str).unwrap_or_default(),
        concept: get(map, "concept").map(NodeExt::py_str),
        unit: get(map, "unit").map(NodeExt::py_str).unwrap_or_default(),
    })
}

fn read_quantity(name: &str, node: &Node, source: &str, at: &Where) -> Result<Quantity> {
    if node.get("concept").is_some() {
        return Err(at.key("concept").error(
            "a quantity is an effect the rules work out, and stands for no taxonomy concept: \
             the concepts are the inputs' (inputs.<name>.concept)",
        ));
    }
    let map = mapping(node, at, &["combine", "parts", "text", "basis"])?;
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
        basis: get(map, "basis")
            .map(|b| read_basis(b, &at.key("basis")))
            .transpose()?,
    })
}

/// A `basis:`: its `kind`, and what it says besides.
fn read_basis(node: &Node, at: &Where) -> Result<Basis> {
    let map = mapping(
        node,
        at,
        &[
            "kind",
            "data",
            "calibration",
            "by",
            "date",
            "reference",
            "text",
        ],
    )?;
    let kind = get(map, "kind").map(NodeExt::py_str).ok_or_else(|| {
        at.key("kind")
            .error(format!("missing: one of {}", Basis::KINDS.join(", ")))
    })?;
    if !Basis::KINDS.contains(&kind.as_str()) {
        return Err(at.key("kind").error(format!(
            "'{kind}' is not one of {}",
            Basis::KINDS.join(", ")
        )));
    }
    if let Some(data) = get(map, "data") {
        let at = at.key("data");
        let data = mapping(data, &at, &["file", "sha256", "rows", "files"])?;
        if let Some(rows) = get(data, "rows") {
            number(rows, &at.key("rows"))?;
        }
        if let Some(files) = get(data, "files") {
            let at = at.key("files");
            let Node::Sequence(items) = untag(files) else {
                return Err(at.error("a list of {file, sha256}"));
            };
            for (k, item) in items.iter().enumerate() {
                mapping(item, &at.index(k), &["file", "sha256"])?;
            }
        }
    }
    if let Some(c) = get(map, "calibration") {
        let at = at.key("calibration");
        for (k, v) in mapping(c, &at, &[])? {
            number(v, &at.key(&k.py_str()))?;
        }
    }
    let mut details = serde_json::Map::new();
    for (k, v) in map {
        let k = k.py_str();
        if k != "kind" {
            details.insert(k, v.to_json());
        }
    }
    Ok(Basis {
        kind,
        details: serde_json::Value::Object(details),
    })
}

fn read_part(name: &str, node: &Node, at: &Where) -> Result<Part> {
    let mut known = vec!["at", "text", "fixed", "basis"];
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
    let fixed = match get(map, "fixed") {
        None => Vec::new(),
        Some(raw) => {
            let here = at.key("fixed");
            match untag(raw) {
            Node::Bool(true) => vec![Hold::ALL],
            Node::Bool(false) | Node::Null => Vec::new(),
            Node::Sequence(items) => items
                .iter()
                .map(|item| match untag(item) {
                    Node::String(spec) => Hold::parse(spec, family, &axes).map_err(|e| here.error(e)),
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
            }
        }
    };
    let part = Part {
        name: name.to_string(),
        axes,
        family,
        values,
        text: get(map, "text").map(NodeExt::py_str).unwrap_or_default(),
        fixed,
        basis: get(map, "basis")
            .map(|b| read_basis(b, &at.key("basis")))
            .transpose()?,
    };
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
fn point_words(part: &Part, k: usize) -> String {
    if part.axes.is_empty() {
        return "everywhere:".into();
    }
    format!("at {}:", point_coords(&part.axes, k).join(", "))
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
        let held = |k, n| holds.iter().any(|h: &Hold| h.holds(k, n));
        assert!(held(1, 2) && held(0, 3) && held(3, 0));
        assert!(!held(0, 1));
        let spec = |h: &Hold| h.spec(part.family, &part.axes).unwrap();
        let specs: Vec<_> = holds.iter().map(spec).collect();
        assert_eq!(specs, ["sd", "mean@a=10.0,b=1.0", "@a=0.0,b=0.0"]);
        for h in holds {
            assert_eq!(&Hold::parse(&spec(h), part.family, &part.axes).unwrap(), h);
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

    /// How many random cases a randomized test runs: `OPENODD_FUZZ_SEEDS`,
    /// else `default` (the weekly fuzz workflow sets it high).
    pub(crate) fn fuzz_seeds(default: u64) -> u64 {
        std::env::var("OPENODD_FUZZ_SEEDS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(default)
    }

    #[test]
    fn random_distributions_read_everywhere() {
        use rand::{Rng, SeedableRng};
        // Any family at any scale; a normal's mean up to 200 standard
        // deviations outside its range: quantiles rise and stay in the
        // range, cdf undoes them, and every summary is a number.
        for seed in 0..fuzz_seeds(2000) {
            let mut rng = rand_chacha::ChaCha8Rng::seed_from_u64(seed);
            let scale = 10f64.powi(rng.gen_range(-8..=8));
            let low = rng.gen_range(-100.0..100.0) * scale;
            let width = 10f64.powf(rng.gen_range(-6.0..2.0)) * scale;
            let high = low + width;
            if !(high > low) {
                continue;
            }
            let d = match rng.gen_range(0..3) {
                0 => Dist::Uniform { low, high },
                1 => Dist::Triangular {
                    low,
                    mode: low + rng.gen::<f64>() * width,
                    high,
                },
                _ => {
                    let sd = 10f64.powf(rng.gen_range(-6.0..2.0)) * scale;
                    Dist::Normal {
                        mean: low + rng.gen_range(-200.0..200.0) * sd,
                        sd,
                        low,
                        high,
                    }
                }
            };
            let mut ps: Vec<f64> = (0..6).map(|_| rng.gen()).collect();
            ps.extend([0.0, 1.0]);
            ps.sort_by(f64::total_cmp);
            let xs: Vec<f64> = ps.iter().map(|p| d.quantile(*p)).collect();
            for (p, x) in ps.iter().zip(&xs) {
                assert!(
                    x.is_finite() && low <= *x && *x <= high,
                    "seed {seed}: {d:?} at {p}: {x}"
                );
                let c = d.cdf(*x);
                assert!(
                    (0.0..=1.0).contains(&c),
                    "seed {seed}: {d:?} cdf({x}) = {c}"
                );
                if width > 1e-9 * low.abs().max(high.abs()) && *p > 1e-6 && *p < 1.0 - 1e-6 {
                    let again = d.quantile(c);
                    assert!(
                        (c - p).abs() < 1e-6 || (x - again).abs() <= 1e-9 * x.abs().max(width),
                        "seed {seed}: {d:?} at {p}: x {x}, cdf {c}"
                    );
                }
                assert!(
                    d.pdf(*x).is_finite() || d.pdf(*x) == f64::INFINITY,
                    "seed {seed}"
                );
            }
            assert!(
                xs.windows(2).all(|w| w[0] <= w[1]),
                "seed {seed}: {d:?}: {xs:?}"
            );
            let m = d.mean();
            assert!(
                m.is_finite() && low <= m && m <= high,
                "seed {seed}: {d:?}: mean {m}"
            );
        }
    }

    #[test]
    fn a_narrow_range_far_in_a_tail_reads_to_its_digits() {
        // Found by fuzzing: 153 standard deviations below the mean, 1e-6
        // wide. The tail's logarithms are near -11700 there, and their
        // difference lost its digits: cdf(quantile(p)) was off by 1e-6.
        let d = Dist::Normal {
            mean: 2295.0,
            sd: 15.0,
            low: 0.0,
            high: 1e-6,
        };
        for p in [1e-6, 0.1, 0.5, 0.9, 1.0 - 1e-6] {
            let x = d.quantile(p);
            assert!((0.0..=1e-6).contains(&x), "{p}: {x}");
            assert!((d.cdf(x) - p).abs() < 1e-9, "{p}: {}", d.cdf(x));
        }
        // All but uniform that narrow: the density over the range all but 1e6.
        assert!((d.pdf(5e-7) - 1e6).abs() < 1e6 * 1e-3, "{}", d.pdf(5e-7));
    }

    #[test]
    fn a_triangular_with_a_subnormal_side_reads() {
        // Found by fuzzing: w * (mode - low) underflowed to 0, and the
        // distribution function read 0 / 0.
        let d = Dist::Triangular {
            low: 0.0,
            mode: 2.22507384e-316,
            high: 1e-8,
        };
        for x in [0.0, 1e-317, 1e-9, 1e-8] {
            let c = d.cdf(x);
            assert!((0.0..=1.0).contains(&c), "{x}: {c}");
            assert!(d.pdf(x).is_finite(), "{x}: {}", d.pdf(x));
        }
    }

    #[test]
    fn a_quantile_rises_from_a_tails_end() {
        // Found by fuzzing: the range four standard deviations above the
        // mean. Near its low end the tail's logarithms were differenced and
        // cancelled: quantile(0) read 1.35e-21 and quantile(2.2e-16) 0.
        let d = Dist::Normal {
            mean: -0.00027044812011718753,
            sd: 6.83125e-05,
            low: 0.0,
            high: 2.9e-05,
        };
        let ps = [
            0.0,
            1e-300,
            1e-16,
            2.220446049250313e-16,
            1e-12,
            1e-8,
            1e-4,
            0.5,
        ];
        let xs: Vec<f64> = ps.iter().map(|p| d.quantile(*p)).collect();
        assert_eq!(xs[0], 0.0);
        assert!(xs.windows(2).all(|w| w[0] <= w[1]), "{xs:?}");
        for (p, x) in ps.iter().zip(&xs).skip(2) {
            assert!((d.cdf(*x) - p).abs() <= 1e-9 * p, "{p}: {}", d.cdf(*x));
        }
    }

    #[test]
    fn a_range_reaching_far_below_the_mean_reads_at_its_low_end() {
        // Found by fuzzing: 38 standard deviations below the mean, phi(a) is
        // subnormal, and the Halley step of inverse_phi overflowed to NaN.
        let d = Dist::Normal {
            mean: 4.75,
            sd: 0.125,
            low: 0.0,
            high: 5.0,
        };
        assert_eq!(d.quantile(0.0), 0.0);
        assert!(d.quantile(1e-300).is_finite());
        assert!((d.quantile(0.5) - 4.746435384176135).abs() < 1e-12);
        assert!(inverse_phi(1e-320).is_finite());
    }

    #[test]
    fn a_triangular_quantile_stays_in_its_range() {
        // Found by fuzzing: low and mode tiny next to high, high - sqrt(...)
        // cancelled to 0, below low.
        let d = Dist::Triangular {
            low: 7.547092539411937e-281,
            mode: 7.547092539411937e-281,
            high: 1.0,
        };
        for p in [0.0, 1e-12, 0.5, 1.0] {
            let x = d.quantile(p);
            assert!((d.support().0..=d.support().1).contains(&x), "{p}: {x}");
        }
        assert_eq!(d.quantile(0.0), 7.547092539411937e-281);
        // Found by fuzzing: a mode tiny next to high; just above it, high -
        // sqrt(...) cancelled to 0, below the quantiles just under it.
        let d = Dist::Triangular {
            low: 0.0,
            mode: 8.612787633870585e-107,
            high: 2000.0,
        };
        let c = 8.612787633870585e-107 / 2000.0;
        let below = d.quantile(c * 0.999);
        let above = d.quantile(c * 1.001);
        assert!(
            below <= above && above >= 8.612787633870585e-107,
            "{below} {above}"
        );
        for p in [c * 0.5, c, c * 2.0, 0.25, 0.75] {
            assert!(
                (d.cdf(d.quantile(p)) - p).abs() <= 1e-12 * p.max(1e-300) + 1e-15,
                "{p}"
            );
        }
        // Found in review: c rounds to 1 with the mode below high.
        let d = Dist::Triangular {
            low: -1.0,
            mode: 0.0,
            high: 1e-20,
        };
        assert_eq!(d.quantile(1.0), 1e-20);
        assert!(d.quantile(0.5).is_finite() && d.quantile(1.0 - 1e-17).is_finite());
        // The mode at an end.
        for mode in [1.0, 0.0] {
            let d = Dist::Triangular {
                low: 0.0,
                mode,
                high: 1.0,
            };
            for p in [0.0, 0.5, 1.0] {
                assert!(d.cdf(d.quantile(p)).is_finite(), "{mode} {p}");
            }
            assert_eq!((d.quantile(0.0), d.quantile(1.0)), (0.0, 1.0));
        }
    }

    #[test]
    fn a_normal_far_from_its_range_still_reads() {
        // The range 0..1 some 38 standard deviations above the mean: its mass
        // underflows, and the distribution is all but an exponential from
        // the low end, at a rate of (low - mean) / sd^2.
        let d = Dist::Normal {
            mean: -0.605,
            sd: 0.0157,
            low: 0.0,
            high: 1.0,
        };
        let rate = 0.605 / (0.0157 * 0.0157);
        for p in [0.0, 1e-9, 0.1, 0.5, 0.9, 1.0 - 1e-9, 1.0] {
            let x = d.quantile(p);
            assert!(x.is_finite() && (0.0..=1.0).contains(&x), "{p}: {x}");
            if p > 0.0 && p < 1.0 {
                assert!((d.cdf(x) - p).abs() < 1e-9, "{p}: cdf {}", d.cdf(x));
                // The exponential holds near the end (it leaves out a
                // quadratic term that grows further in).
                let exponential = -(1.0 - p).ln() / rate;
                assert!(
                    p > 0.9 || (x - exponential).abs() < 2e-3 * exponential.max(1e-12) + 1e-12,
                    "{p}: {x} vs {exponential}"
                );
            }
        }
        assert_eq!(d.quantile(0.0), 0.0);
        assert!(
            d.pdf(0.0).is_finite() && (d.pdf(0.0) - rate).abs() < 1e-3 * rate,
            "{}",
            d.pdf(0.0)
        );
        assert!(
            d.mean().is_finite() && (d.mean() - 1.0 / rate).abs() < 2e-3 / rate,
            "{}",
            d.mean()
        );
        // Mirrored: a range far below the mean.
        let m = Dist::Normal {
            mean: 0.605,
            sd: 0.0157,
            low: -1.0,
            high: 0.0,
        };
        for p in [0.1, 0.5, 0.9] {
            assert!((m.quantile(p) + d.quantile(1.0 - p)).abs() < 1e-12, "{p}");
            assert!((m.cdf(m.quantile(p)) - p).abs() < 1e-9);
        }
        // Either side of where the tail's series takes over (z = 35), the
        // same as the closed form gives.
        for a in [34.0, 34.99, 35.01, 36.0] {
            let d = Dist::Normal {
                mean: -a,
                sd: 1.0,
                low: 0.0,
                high: 1.0,
            };
            let x = d.quantile(0.5);
            assert!((d.cdf(x) - 0.5).abs() < 1e-9, "{a}");
            // Exponential at rate a, its quadratic corrected: within 1/a^2.
            assert!(
                (x - 2f64.ln() / a).abs() < 1.5 * 2f64.ln() / (a * a * a),
                "{a}: {x}"
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
            (GRIP.replace("version: 2", "version: 4"), "version must be 2 to 3"),
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

    const TAXONOMY_URL: &str = "https://example.com/taxonomy.yaml";
    const ROAD_TAXONOMY: &str = "TAXONOMY:\n  weather:\n    rainfall_rate: float precipitation_rate\n    rain: [none, some]\n  road:\n    friction: float\n";

    fn with_taxonomy(text: &str) -> Result<RuleSet> {
        let mut map = IndexMap::new();
        map.insert(TAXONOMY_URL.to_string(), ROAD_TAXONOMY.to_string());
        RuleSet::parse_with(text, "x.yaml", &Origin::Text, &mut Given(map))
    }

    const V3: &str = "version: 3\ntaxonomy: https://example.com/taxonomy.yaml\nbasis: {kind: expert, by: tests}\ninputs:\n  rainfall_rate: {concept: weather.rainfall_rate, unit: mm/h}\nquantities:\n  grip:\n    parts:\n      rain:\n        at: {rainfall_rate: [0, 10]}\n        uniform: {low: [0.9, 0.5], high: [1.1, 0.7]}\n";

    #[test]
    fn version_3_inputs_stand_for_taxonomy_concepts() {
        let set = with_taxonomy(V3).unwrap();
        assert_eq!(set.version, 3);
        let taxonomy = set.taxonomy.as_ref().unwrap();
        assert_eq!(taxonomy.label, TAXONOMY_URL);
        assert_eq!(
            set.inputs["rainfall_rate"].concept.as_deref(),
            Some("weather.rainfall_rate")
        );
        assert!(set.unclassified_inputs().is_empty());
        let mut at = IndexMap::new();
        at.insert("rainfall_rate".to_string(), 10.0);
        assert_eq!(set.quantile("grip", &at, 0.0).unwrap(), 0.5);
    }

    #[test]
    fn version_3_refuses_inputs_that_are_no_concept() {
        let err = |text: &str| with_taxonomy(text).unwrap_err().to_string();
        let no_concept = V3.replace(
            "  rainfall_rate: {concept: weather.rainfall_rate, unit: mm/h}\n",
            "  rainfall_rate: {default: 0}\n",
        );
        let e = err(&no_concept);
        assert!(e.contains("stands for no taxonomy concept"), "{e}");
        let undeclared = V3.replace(
            "inputs:\n  rainfall_rate: {concept: weather.rainfall_rate, unit: mm/h}\n",
            "",
        );
        assert!(err(&undeclared).contains("'rainfall_rate' is read by a part"));
        let unknown = V3.replace("weather.rainfall_rate", "weather.rain_rate");
        let e = err(&unknown);
        assert!(
            e.contains("'weather.rain_rate' is not a concept of the taxonomy"),
            "{e}"
        );
        let categorical = V3.replace("weather.rainfall_rate", "weather.rain");
        assert!(err(&categorical).contains("is not a number"));
        let wrong_unit = V3.replace("unit: mm/h", "unit: km/h");
        let e = err(&wrong_unit);
        assert!(
            e.contains("inputs.rainfall_rate.unit") && e.contains("measures velocity"),
            "{e}"
        );
        let quantity_concept = V3.replace(
            "  grip:\n    parts:",
            "  grip:\n    concept: road.friction\n    parts:",
        );
        assert!(err(&quantity_concept).contains("a quantity is an effect"));
        let no_taxonomy = V3.replace("taxonomy: https://example.com/taxonomy.yaml\n", "");
        assert!(err(&no_taxonomy).contains("version 3 names the taxonomy"));
    }

    #[test]
    fn version_2_inputs_are_unclassified() {
        let set = RuleSet::parse(GRIP, "grip.yaml").unwrap();
        assert_eq!(set.version, 2);
        assert_eq!(set.unclassified_inputs(), ["water"]);
        let e = with_taxonomy(&V3.replace("version: 3", "version: 2"))
            .unwrap_err()
            .to_string();
        assert!(e.contains("a taxonomy is named from version 3"), "{e}");
    }

    #[test]
    fn a_version_2_overlay_keeps_a_version_3_set() {
        let set = with_taxonomy(V3).unwrap();
        let over = "version: 2\nquantities:\n  grip:\n    parts:\n      rain:\n        uniform: {low: [0.8, 0.4], high: [1.0, 0.6]}\n";
        let merged = set
            .parse_over(over, "fit.yaml", &Origin::Text, &mut Given(IndexMap::new()))
            .unwrap();
        assert_eq!(merged.version, 3);
        assert!(merged.taxonomy.is_some());
        assert!(merged.unclassified_inputs().is_empty());
    }

    #[test]
    fn the_taxonomy_is_fetched_ahead_too() {
        let refs = RuleSet::references(V3, "x.yaml", &Origin::Text).unwrap();
        assert_eq!(
            refs.iter().map(Origin::label).collect::<Vec<_>>(),
            [TAXONOMY_URL]
        );
        // apart from the rule files extended, which a host follows in turn
        assert!(RuleSet::extends_references(V3, "x.yaml", &Origin::Text)
            .unwrap()
            .is_empty());
        assert_eq!(
            RuleSet::taxonomy_reference(V3, "x.yaml", &Origin::Text)
                .unwrap()
                .map(|o| o.label()),
            Some(TAXONOMY_URL.to_string())
        );
    }

    #[test]
    fn a_part_says_what_its_values_rest_on() {
        let text = V3.replace(
            "        uniform: {low: [0.9, 0.5], high: [1.1, 0.7]}\n",
            "        uniform: {low: [0.9, 0.5], high: [1.1, 0.7]}\n        basis: {kind: fitted, data: {file: drives.csv, sha256: abc, rows: 120}, calibration: {in_band: 0.88}}\n",
        );
        let set = with_taxonomy(&text).unwrap();
        let b = set.basis_of("grip", "rain").unwrap();
        assert_eq!(b.kind, "fitted");
        assert_eq!(b.to_json()["data"]["rows"], 120);
        assert_eq!(b.to_json()["calibration"]["in_band"], 0.88);
        // Without its own, the file's.
        let set = with_taxonomy(V3).unwrap();
        assert_eq!(set.basis_of("grip", "rain").unwrap().kind, "expert");
        assert_eq!(
            set.basis_of("grip", "rain").unwrap().to_json()["by"],
            "tests"
        );
        // and the quantity's before the file's.
        let text = V3.replace(
            "  grip:\n    parts:",
            "  grip:\n    basis: {kind: literature, reference: some paper}\n    parts:",
        );
        assert_eq!(
            with_taxonomy(&text)
                .unwrap()
                .basis_of("grip", "rain")
                .unwrap()
                .kind,
            "literature"
        );
    }

    #[test]
    fn version_3_refuses_a_part_resting_on_nothing() {
        let e = with_taxonomy(&V3.replace("basis: {kind: expert, by: tests}\n", ""))
            .unwrap_err()
            .to_string();
        assert!(e.contains("quantities.grip.parts.rain: no basis"), "{e}");
        let e = with_taxonomy(&V3.replace("kind: expert", "kind: guess"))
            .unwrap_err()
            .to_string();
        assert!(
            e.contains("'guess' is not one of fitted, expert, literature"),
            "{e}"
        );
        let e = with_taxonomy(&V3.replace("by: tests", "by: tests, rows: 3"))
            .unwrap_err()
            .to_string();
        assert!(e.contains("unknown fields"), "{e}");
        // Version 2 parts may rest on nothing said.
        let set = RuleSet::parse(GRIP, "grip.yaml").unwrap();
        assert!(set.quantities["grip"].parts[0].basis.is_none());
        assert!(set.basis_of("grip", "water").is_none());
    }

    #[test]
    fn a_taxonomy_that_imports_others_is_refused() {
        let mut map = IndexMap::new();
        map.insert(
            TAXONOMY_URL.to_string(),
            format!("IMPORT: [more.yaml]\n{ROAD_TAXONOMY}"),
        );
        let e = RuleSet::parse_with(V3, "x.yaml", &Origin::Text, &mut Given(map))
            .unwrap_err()
            .to_string();
        assert!(
            e.contains("is one OpenODD file, and this one IMPORTs others"),
            "{e}"
        );
    }
}
