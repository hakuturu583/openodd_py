//! Find flaky cases: where a run's pass/fail outcome flips under small changes.
//!
//! A test that passes at one setting and fails a hair away -- or passes and
//! fails at the same setting -- is unstable, and the ODD's coverage alone does
//! not find it: coverage spreads runs over the buckets, while instability sits
//! on the boundary between outcomes. This module models that boundary.
//!
//! **The model** is Gaussian-process classification. The parameters of a case
//! are put in a normalized space ([`ParamSpace`]): a numeric parameter scaled
//! to `[0, 1]` over its range, a categorical one as a one-hot code scaled so
//! that two categories are 1 apart. A latent function `f ~ GP(0, k)`, with a
//! squared-exponential kernel `k`, gives `P(pass | f) = Φ(f)` (probit). The
//! posterior over `f` is the Laplace approximation (Rasmussen & Williams,
//! *Gaussian Processes for Machine Learning*, algorithms 3.1 and 3.2). The
//! kernel has a length scale per parameter (automatic relevance
//! determination: a parameter the outcome does not depend on ends up with a
//! long one). They start from the length scale of an isotropic GP regression
//! on the ±1 outcomes fitted by [`friedrich`] (marginal likelihood, gradient
//! descent); each, and the amplitude, are then refined in turn by the Laplace
//! approximation's own evidence.
//!
//! **The metrics** of a point `x` ([`Metrics`]):
//!
//! * `flip_probability` -- the chance that a run at `x` and one at a small
//!   change of it (`x'` uniform in the ball of radius `epsilon` about `x`,
//!   numeric parameters only) disagree, averaged over the posterior: with
//!   `p = Φ(f(x))` and `p' = Φ(f(x'))`, `E[p (1 - p') + (1 - p) p']`. It counts
//!   a setting whose outcome is a coin toss as flaky too (`p' = p = 0.5`).
//! * `instability_confidence` -- the posterior probability that that flip rate
//!   exceeds `threshold`: the share of joint posterior draws of `f` at `x` and
//!   its neighbours whose flip rate does.
//! * `robustness_radius` -- the smallest normalized distance from `x` at which
//!   the predicted outcome (the sign of the posterior mean of `f`) changes:
//!   searched along each numeric axis both ways and along random directions,
//!   to the edge of the space, then refined by bisection; switching `k`
//!   categorical parameters is a step of `√k` (every combination of them is
//!   tried while there are at most 1024). `None` when nothing searched
//!   changes it: the radius is the smallest change found, so an estimate
//!   from above.
//!
//! **The search** ([`search`]) keeps the sampler's own policy -- uniform, or
//! the coverage strategy's least covered first -- for part of a batch, and
//! fills the rest with the candidates of highest instability: drawn from the
//! ODD by a copy of the sampler (so only admitted combinations), scored, and
//! kept apart by at least `epsilon`.

use std::collections::HashMap;

use indexmap::IndexMap;
use nalgebra::{Cholesky, DMatrix, DVector, Dyn};
use rand::Rng;
use rand_chacha::ChaCha8Rng;

use crate::error::{Error, Result};
use crate::sampler::{rng_for, Domain, Sample, Sampler};
use crate::value::Value;

/// A one-hot code is scaled by this, so two categories are 1 apart.
const CATEGORY_SCALE: f64 = std::f64::consts::FRAC_1_SQRT_2;

// ---------------------------------------------------------------------------
// The normalized space
// ---------------------------------------------------------------------------

/// One parameter of the space.
#[derive(Debug, Clone, PartialEq)]
pub enum Dim {
    /// A number, scaled to `[0, 1]` over `[low, high]`.
    Numeric { name: String, low: f64, high: f64 },
    /// One of `labels`, one-hot coded.
    Categorical { name: String, labels: Vec<String> },
}

impl Dim {
    pub fn name(&self) -> &str {
        match self {
            Dim::Numeric { name, .. } | Dim::Categorical { name, .. } => name,
        }
    }

    fn width(&self) -> usize {
        match self {
            Dim::Numeric { .. } => 1,
            Dim::Categorical { labels, .. } => labels.len(),
        }
    }
}

/// The parameters of a case, normalized so that parameters of any unit or
/// kind can be measured against each other.
#[derive(Debug, Clone, PartialEq)]
pub struct ParamSpace {
    dims: Vec<Dim>,
}

impl ParamSpace {
    pub fn new(dims: Vec<Dim>) -> Self {
        ParamSpace { dims }
    }

    /// The space the attributes *sampler* draws span: a numeric attribute
    /// over what its knob can bring about in its drawable buckets, a
    /// categorical one over its drawable buckets.
    pub fn of(sampler: &Sampler) -> Self {
        let dims = sampler
            .domains()
            .into_iter()
            .map(|(name, domain)| match domain {
                Domain::Numeric { low, high } => Dim::Numeric { name, low, high },
                Domain::Labels(labels) => Dim::Categorical { name, labels },
            })
            .collect();
        ParamSpace::new(dims)
    }

    /// Refuse a space that cannot code a case.
    pub fn check(&self) -> Result<()> {
        if self.dims.is_empty() {
            return Err(Error::value("ParamSpace: no parameters"));
        }
        let mut names = std::collections::HashSet::new();
        for dim in &self.dims {
            if !names.insert(dim.name()) {
                return Err(Error::value(format!(
                    "ParamSpace: {} is named twice",
                    dim.name()
                )));
            }
            match dim {
                Dim::Numeric { name, low, high } => {
                    if !(low.is_finite() && high.is_finite() && low <= high) {
                        return Err(Error::value(format!(
                            "ParamSpace: {name} needs a finite [low, high], not [{low}, {high}]"
                        )));
                    }
                }
                Dim::Categorical { name, labels } => {
                    let distinct: std::collections::HashSet<&String> = labels.iter().collect();
                    if labels.is_empty() || distinct.len() != labels.len() {
                        return Err(Error::value(format!(
                            "ParamSpace: {name} needs distinct labels"
                        )));
                    }
                }
            }
        }
        Ok(())
    }

    pub fn dims(&self) -> &[Dim] {
        &self.dims
    }

    /// The length of a coded case.
    pub fn width(&self) -> usize {
        self.dims.iter().map(Dim::width).sum()
    }

    /// A case's values, by parameter name, coded: a number scaled over its
    /// range (not clipped: a case outside it is outside `[0, 1]`), a category
    /// one-hot.
    pub fn encode(&self, values: &IndexMap<String, Value>) -> Result<Vec<f64>> {
        let mut out = Vec::with_capacity(self.width());
        for dim in &self.dims {
            let value = values
                .get(dim.name())
                .ok_or_else(|| Error::value(format!("{}: no value", dim.name())))?;
            match dim {
                Dim::Numeric { name, low, high } => {
                    let x = value.number().filter(|x| x.is_finite()).ok_or_else(|| {
                        Error::value(format!("{name}: {} is not a number", value.label()))
                    })?;
                    let span = high - low;
                    out.push(if span > 0.0 { (x - low) / span } else { 0.5 });
                }
                Dim::Categorical { name, labels } => {
                    let label = value.label();
                    let index = labels.iter().position(|l| *l == label).ok_or_else(|| {
                        Error::value(format!("{name}: {label} is none of {labels:?}"))
                    })?;
                    out.extend((0..labels.len()).map(|i| {
                        if i == index {
                            CATEGORY_SCALE
                        } else {
                            0.0
                        }
                    }));
                }
            }
        }
        Ok(out)
    }

    /// The parameter of each coordinate of a coded case.
    fn groups(&self) -> Vec<usize> {
        self.dims
            .iter()
            .enumerate()
            .flat_map(|(p, d)| std::iter::repeat_n(p, d.width()))
            .collect()
    }

    /// The numeric parameters that can change, `(coordinate, name, low,
    /// high)`. A parameter fixed at one value (`low == high`) cannot, so it is
    /// left out.
    fn numeric_dims(&self) -> Vec<(usize, &str, f64, f64)> {
        let mut out = Vec::new();
        let mut at = 0;
        for dim in &self.dims {
            if let Dim::Numeric { name, low, high } = dim {
                if high > low {
                    out.push((at, name.as_str(), *low, *high));
                }
            }
            at += dim.width();
        }
        out
    }

    /// The coordinates of the numeric parameters that can change.
    fn numeric_coords(&self) -> Vec<usize> {
        self.numeric_dims().into_iter().map(|(c, ..)| c).collect()
    }

    /// The label set in each categorical parameter of a coded case.
    fn categories(&self, x: &[f64]) -> Vec<Option<usize>> {
        self.categorical_blocks()
            .into_iter()
            .map(|(at, n)| (0..n).position(|i| x[at + i] > 0.0))
            .collect()
    }

    /// `(first coordinate, number of labels)` of each categorical parameter.
    fn categorical_blocks(&self) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        let mut at = 0;
        for dim in &self.dims {
            if let Dim::Categorical { labels, .. } = dim {
                out.push((at, labels.len()));
            }
            at += dim.width();
        }
        out
    }
}

// ---------------------------------------------------------------------------
// The standard normal
// ---------------------------------------------------------------------------

fn norm_pdf(z: f64) -> f64 {
    (-0.5 * z * z).exp() / (2.0 * std::f64::consts::PI).sqrt()
}

/// The complementary error function (Numerical Recipes' `erfcc`: relative
/// error below 1.2e-7 everywhere).
fn erfc(x: f64) -> f64 {
    let z = x.abs();
    let t = 1.0 / (1.0 + 0.5 * z);
    let ans = t
        * (-z * z - 1.265_512_23
            + t * (1.000_023_68
                + t * (0.374_091_96
                    + t * (0.096_784_18
                        + t * (-0.186_288_06
                            + t * (0.278_868_07
                                + t * (-1.135_203_98
                                    + t * (1.488_515_87
                                        + t * (-0.822_152_23 + t * 0.170_872_77)))))))))
            .exp();
    if x >= 0.0 {
        ans
    } else {
        2.0 - ans
    }
}

/// Φ, the standard normal distribution function.
pub fn norm_cdf(z: f64) -> f64 {
    0.5 * erfc(-z / std::f64::consts::SQRT_2)
}

/// φ(z) / Φ(z), kept accurate far in the lower tail.
fn inverse_mills(z: f64) -> f64 {
    if z < -6.0 {
        let r = -z;
        let r2 = r * r;
        r / (1.0 - 1.0 / r2 + 3.0 / (r2 * r2) - 15.0 / (r2 * r2 * r2))
    } else {
        norm_pdf(z) / norm_cdf(z)
    }
}

fn log_cdf(z: f64) -> f64 {
    if z < -6.0 {
        // ln φ(z) written out: φ itself underflows far in the tail.
        -0.5 * z * z - 0.5 * (2.0 * std::f64::consts::PI).ln() - inverse_mills(z).ln()
    } else {
        norm_cdf(z).ln()
    }
}

/// A standard normal draw (Box-Muller).
fn standard_normal(rng: &mut ChaCha8Rng) -> f64 {
    let u1: f64 = 1.0 - rng.gen::<f64>(); // (0, 1]
    let u2: f64 = rng.gen();
    (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
}

// ---------------------------------------------------------------------------
// The classifier
// ---------------------------------------------------------------------------

/// The largest amplitude (latent variance) the fit chooses: a latent standard
/// deviation of 4, where Φ is within 3e-5 of 0 or 1.
const MAX_AMPLITUDE: f64 = 16.0;

/// A log-normal prior on a length scale, in the normalized space, and the
/// range it is kept in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LengthScalePrior {
    pub categorical: bool,
    /// The prior's median.
    pub median: f64,
    /// The standard deviation of the log length scale.
    pub log_sd: f64,
    pub min: f64,
    pub max: f64,
}

impl LengthScalePrior {
    /// A numeric parameter: a boundary that bends over about a third of its
    /// range, give or take a factor of e; from 1% to 10 ranges.
    pub const NUMERIC: LengthScalePrior = LengthScalePrior {
        categorical: false,
        median: 0.3,
        log_sd: 1.0,
        min: 0.01,
        max: 10.0,
    };
    /// A categorical parameter: two categories (1 apart) correlated by
    /// e^(-1/2) at the median. Kept below 4 (a correlation of 0.97 between
    /// categories): the model may find a category nearly irrelevant, never
    /// entirely, so runs in one category still say something of another
    /// while each category's own boundary is learnt.
    pub const CATEGORICAL: LengthScalePrior = LengthScalePrior {
        categorical: true,
        median: 1.0,
        log_sd: 1.0,
        min: 0.05,
        max: 4.0,
    };

    fn clamp(&self, ls: f64) -> f64 {
        ls.clamp(self.min, self.max)
    }

    /// The log density of the log length scale, up to a constant.
    fn log_density(&self, ls: f64) -> f64 {
        let z = (ls.ln() - self.median.ln()) / self.log_sd;
        -0.5 * z * z
    }
}

/// The squared-exponential kernel with a length scale per coordinate (ARD).
fn kernel(a: &[f64], b: &[f64], ls: &[f64], ampl: f64) -> f64 {
    let d2: f64 = a
        .iter()
        .zip(b)
        .zip(ls)
        .map(|((x, y), l)| ((x - y) / l).powi(2))
        .sum();
    ampl * (-0.5 * d2).exp()
}

/// Laplace-approximated GP classification with a probit likelihood.
#[derive(Debug, Clone)]
pub struct Classifier {
    x: Vec<Vec<f64>>,
    /// The length scale of each parameter, in the normalized space: long for
    /// one the outcome does not depend on.
    pub length_scales: Vec<f64>,
    /// The length scale of each coordinate of a coded case.
    coordinate_scales: Vec<f64>,
    /// Squared-exponential amplitude (the latent variance).
    pub amplitude: f64,
    /// ∇ log p(y | f) at the posterior mode.
    grad: DVector<f64>,
    sqrt_w: DVector<f64>,
    /// Lower Cholesky factor of B = I + W^½ K W^½.
    l: DMatrix<f64>,
    /// The approximate log marginal likelihood.
    pub log_evidence: f64,
}

impl Classifier {
    /// Fit the classifier to coded cases and their outcomes. `groups` maps
    /// each coordinate of a coded case to its parameter, which has one length
    /// scale (a categorical parameter's one-hot coordinates share one).
    ///
    /// The length scales start from friedrich's fit of an isotropic GP
    /// regression on the ±1 outcomes; then each parameter's length scale and
    /// the amplitude are refined in turn by the Laplace evidence.
    ///
    /// The refinement maximizes the evidence plus a log-normal prior on each
    /// length scale ([`LengthScalePrior`]): with few runs the evidence alone
    /// can call a parameter irrelevant (a length scale of 100) that only
    /// matters near the boundary, where the runs are not yet; the prior keeps
    /// it within reach of the data until enough runs say otherwise.
    /// `categorical[p]` says whether parameter `p` is categorical.
    pub fn fit(
        x: &[Vec<f64>],
        passed: &[bool],
        groups: &[usize],
        categorical: &[bool],
    ) -> Result<Self> {
        if x.is_empty() || x.len() != passed.len() {
            return Err(Error::value("flaky: no observations to fit"));
        }
        if x.iter().any(|row| row.len() != groups.len()) {
            return Err(Error::value("flaky: a coded case of the wrong width"));
        }
        let y: Vec<f64> = passed.iter().map(|p| if *p { 1.0 } else { -1.0 }).collect();
        let params = groups.iter().max().map_or(0, |m| m + 1);
        if categorical.len() != params {
            return Err(Error::value("flaky: a kind for every parameter"));
        }
        let priors: Vec<LengthScalePrior> = categorical
            .iter()
            .map(|c| {
                if *c {
                    LengthScalePrior::CATEGORICAL
                } else {
                    LengthScalePrior::NUMERIC
                }
            })
            .collect();
        let log_prior =
            |ls: &[f64]| -> f64 { ls.iter().zip(&priors).map(|(l, p)| p.log_density(*l)).sum() };
        let base = regression_length_scale(x, &y)
            .unwrap_or(0.3)
            .clamp(0.02, 3.0);
        let attempt = |ls: &[f64], ampl: f64| {
            let coordinates: Vec<f64> = groups.iter().map(|g| ls[*g]).collect();
            laplace(x, &y, ls.to_vec(), coordinates, ampl)
        };
        // A numeric parameter starts from friedrich's length scale, a
        // categorical one from its prior's median.
        let mut ls: Vec<f64> = priors
            .iter()
            .map(|p| {
                if p.categorical {
                    p.median
                } else {
                    p.clamp(base)
                }
            })
            .collect();
        let mut ampl = 4.0;
        let mut best = attempt(&ls, ampl)
            .or_else(|| {
                ls = priors.iter().map(|p| p.median).collect();
                ampl = 1.0;
                attempt(&ls, ampl)
            })
            .ok_or_else(|| Error::value("flaky: the classifier did not converge"))?;
        let mut best_objective = best.log_evidence + log_prior(&ls);
        let passes = if x.len() <= 400 { 3 } else { 1 };
        for _ in 0..passes {
            let mut improved = false;
            for p in 0..params {
                for factor in [0.25, 0.5, 2.0, 4.0, 16.0] {
                    let mut candidate = ls.clone();
                    candidate[p] = priors[p].clamp(ls[p] * factor);
                    if let Some(c) = attempt(&candidate, ampl) {
                        let objective = c.log_evidence + log_prior(&candidate);
                        if objective > best_objective + 1e-9 {
                            best = c;
                            best_objective = objective;
                            ls = candidate;
                            improved = true;
                        }
                    }
                }
            }
            for factor in [0.25, 0.5, 2.0, 4.0] {
                // Capped: on separable outcomes the evidence keeps rising with
                // the amplitude, and a huge prior variance far from the runs
                // would pull every prediction there back to a coin toss.
                let candidate = (ampl * factor).clamp(0.25, MAX_AMPLITUDE);
                if let Some(c) = attempt(&ls, candidate) {
                    let objective = c.log_evidence + log_prior(&ls);
                    if objective > best_objective + 1e-9 {
                        best = c;
                        best_objective = objective;
                        ampl = candidate;
                        improved = true;
                    }
                }
            }
            if !improved {
                break;
            }
        }
        Ok(best)
    }

    fn cross(&self, points: &[Vec<f64>]) -> DMatrix<f64> {
        DMatrix::from_fn(self.x.len(), points.len(), |i, j| {
            kernel(
                &self.x[i],
                &points[j],
                &self.coordinate_scales,
                self.amplitude,
            )
        })
    }

    /// The posterior mean of the latent function at *points*.
    pub fn latent_mean(&self, points: &[Vec<f64>]) -> Vec<f64> {
        (self.cross(points).transpose() * &self.grad)
            .iter()
            .copied()
            .collect()
    }

    /// The posterior mean and covariance of the latent function at *points*.
    pub fn latent(&self, points: &[Vec<f64>]) -> (DVector<f64>, DMatrix<f64>) {
        let ks = self.cross(points);
        let mean = ks.transpose() * &self.grad;
        let mut v = ks;
        for (i, mut row) in v.row_iter_mut().enumerate() {
            row *= self.sqrt_w[i];
        }
        let v = self
            .l
            .solve_lower_triangular(&v)
            .expect("the Cholesky factor has a positive diagonal");
        let kss = DMatrix::from_fn(points.len(), points.len(), |i, j| {
            kernel(
                &points[i],
                &points[j],
                &self.coordinate_scales,
                self.amplitude,
            )
        });
        (mean, kss - v.transpose() * v)
    }

    /// P(pass) at *point*, the latent function integrated out.
    pub fn pass_probability(&self, point: &[f64]) -> f64 {
        let (mean, cov) = self.latent(&[point.to_vec()]);
        norm_cdf(mean[0] / (1.0 + cov[(0, 0)].max(0.0)).sqrt())
    }
}

/// The length scale of a GP regression on the ±1 outcomes, fitted by
/// friedrich; `None` when there is too little to fit or the fit fails.
fn regression_length_scale(x: &[Vec<f64>], y: &[f64]) -> Option<f64> {
    use friedrich::gaussian_process::GaussianProcess;
    use friedrich::kernel::SquaredExp;
    use friedrich::prior::ConstantPrior;

    if x.len() < 3 {
        return None;
    }
    let (inputs, outputs) = (x.to_vec(), y.to_vec());
    let fitted = std::panic::catch_unwind(move || {
        GaussianProcess::builder(inputs, outputs)
            .set_kernel(SquaredExp::default())
            .set_prior(ConstantPrior::new(0.0))
            .set_noise(0.1)
            .set_cholesky_epsilon(Some(1e-8))
            .fit_kernel()
            .train()
            .kernel
            .ls
    })
    .ok()?;
    let ls = fitted.abs();
    (ls.is_finite() && ls > 0.0).then_some(ls)
}

/// The Laplace approximation's posterior mode and factors (R&W algorithm
/// 3.1, probit likelihood); `None` if a factorization fails.
fn laplace(
    x: &[Vec<f64>],
    y: &[f64],
    length_scales: Vec<f64>,
    coordinate_scales: Vec<f64>,
    ampl: f64,
) -> Option<Classifier> {
    let n = x.len();
    let k = DMatrix::from_fn(n, n, |i, j| kernel(&x[i], &x[j], &coordinate_scales, ampl));
    let y = DVector::from_column_slice(y);
    let mut f = DVector::<f64>::zeros(n);
    let mut previous = f64::NEG_INFINITY;
    let derivatives = |f: &DVector<f64>| {
        let mut grad = DVector::zeros(n);
        let mut w = DVector::zeros(n);
        for i in 0..n {
            let z = y[i] * f[i];
            let m = inverse_mills(z);
            grad[i] = y[i] * m;
            w[i] = (m * m + z * m).max(1e-12);
        }
        (grad, w)
    };
    let factor = |w: &DVector<f64>| {
        let sw = w.map(f64::sqrt);
        let mut b = k.clone();
        for i in 0..n {
            for j in 0..n {
                b[(i, j)] *= sw[i] * sw[j];
            }
            b[(i, i)] += 1.0;
        }
        Cholesky::<f64, Dyn>::new(b).map(|c| (sw, c.l()))
    };
    for _ in 0..100 {
        let (grad, w) = derivatives(&f);
        let (sw, l) = factor(&w)?;
        let b = w.component_mul(&f) + &grad;
        let c = (&k * &b).component_mul(&sw);
        let v = l.solve_lower_triangular(&c)?;
        let u = l.transpose().solve_upper_triangular(&v)?;
        let a = b - sw.component_mul(&u);
        f = &k * &a;
        let objective = -0.5 * a.dot(&f)
            + f.iter()
                .zip(y.iter())
                .map(|(fi, yi)| log_cdf(yi * fi))
                .sum::<f64>();
        if !objective.is_finite() {
            return None;
        }
        if (objective - previous).abs() < 1e-9 {
            break;
        }
        previous = objective;
    }
    let (grad, w) = derivatives(&f);
    let (sqrt_w, l) = factor(&w)?;
    // At the mode a = ∇ log p(y | f), and f = K a.
    let log_evidence = -0.5 * grad.dot(&f)
        + f.iter()
            .zip(y.iter())
            .map(|(fi, yi)| log_cdf(yi * fi))
            .sum::<f64>()
        - l.diagonal().iter().map(|d| d.ln()).sum::<f64>();
    log_evidence.is_finite().then(|| Classifier {
        x: x.to_vec(),
        length_scales,
        coordinate_scales,
        amplitude: ampl,
        grad,
        sqrt_w,
        l,
        log_evidence,
    })
}

// ---------------------------------------------------------------------------
// The metrics
// ---------------------------------------------------------------------------

/// How flaky a point is (see the module docs).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Metrics {
    /// P(pass) at the point, the latent function integrated out.
    pub pass_probability: f64,
    /// The estimated chance a small change flips the outcome.
    pub flip_probability: f64,
    /// The posterior probability that the flip rate exceeds the threshold.
    pub instability_confidence: f64,
    /// The smallest normalized distance at which the predicted outcome changes.
    pub robustness_radius: Option<f64>,
}

/// What the metrics are measured with.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Config {
    /// The flip rate above which a point is unstable.
    pub threshold: f64,
    /// The radius of a small change, in the normalized space.
    pub epsilon: f64,
    /// Neighbours drawn within `epsilon`.
    pub perturbations: usize,
    /// Joint posterior draws.
    pub draws: usize,
    /// Random directions searched for the robustness radius, besides the axes.
    pub directions: usize,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            threshold: 0.1,
            epsilon: 0.05,
            perturbations: 32,
            draws: 256,
            directions: 16,
        }
    }
}

impl Config {
    pub fn check(&self) -> Result<()> {
        if !(0.0..1.0).contains(&self.threshold) {
            return Err(Error::value("flaky: threshold must be in [0, 1)"));
        }
        if !(self.epsilon > 0.0 && self.epsilon.is_finite()) {
            return Err(Error::value("flaky: epsilon must be positive"));
        }
        if self.perturbations == 0 || self.draws == 0 {
            return Err(Error::value(
                "flaky: perturbations and draws must be positive",
            ));
        }
        Ok(())
    }
}

/// A Cholesky factor of a covariance matrix, with as little jitter as needed.
fn jittered_cholesky(mut cov: DMatrix<f64>) -> DMatrix<f64> {
    let n = cov.nrows();
    let scale = (0..n).map(|i| cov[(i, i)].abs()).fold(1e-12, f64::max);
    let mut jitter = 1e-10 * scale;
    for _ in 0..12 {
        if let Some(c) = Cholesky::new(cov.clone()) {
            return c.l();
        }
        for i in 0..n {
            cov[(i, i)] += jitter;
        }
        jitter *= 10.0;
    }
    // Hopeless: keep only the variances.
    DMatrix::from_fn(n, n, |i, j| {
        if i == j {
            cov[(i, i)].max(0.0).sqrt()
        } else {
            0.0
        }
    })
}

/// A classifier, the space it codes cases in, and the observations it is fitted to.
#[derive(Debug, Clone)]
pub struct FlakinessModel {
    space: ParamSpace,
    pub config: Config,
    seed: String,
    x: Vec<Vec<f64>>,
    passed: Vec<bool>,
    classifier: Option<Classifier>,
}

impl FlakinessModel {
    pub fn new(space: ParamSpace, config: Config, seed: &str) -> Result<Self> {
        space.check()?;
        config.check()?;
        Ok(FlakinessModel {
            space,
            config,
            seed: seed.to_string(),
            x: Vec::new(),
            passed: Vec::new(),
            classifier: None,
        })
    }

    pub fn space(&self) -> &ParamSpace {
        &self.space
    }

    pub fn len(&self) -> usize {
        self.x.len()
    }

    pub fn is_empty(&self) -> bool {
        self.x.is_empty()
    }

    /// Record one run: its values, by parameter name, and whether it passed.
    pub fn observe(&mut self, values: &IndexMap<String, Value>, passed: bool) -> Result<()> {
        let x = self.space.encode(values)?;
        self.x.push(x);
        self.passed.push(passed);
        self.classifier = None;
        Ok(())
    }

    /// The fitted classifier (fitted again after new observations).
    pub fn classifier(&mut self) -> Result<&Classifier> {
        if self.classifier.is_none() {
            let groups = self.space.groups();
            let categorical: Vec<bool> = self
                .space
                .dims()
                .iter()
                .map(|d| matches!(d, Dim::Categorical { .. }))
                .collect();
            self.classifier = Some(Classifier::fit(
                &self.x,
                &self.passed,
                &groups,
                &categorical,
            )?);
        }
        Ok(self.classifier.as_ref().expect("fitted above"))
    }

    /// The metrics of a case, by its values.
    pub fn metrics(&mut self, values: &IndexMap<String, Value>) -> Result<Metrics> {
        let x = self.space.encode(values)?;
        self.metrics_at(&x)
    }

    /// The metrics of a coded case.
    pub fn metrics_at(&mut self, x: &[f64]) -> Result<Metrics> {
        let config = self.config;
        self.measure(x, config.perturbations, config.draws, true)
    }

    fn measure(
        &mut self,
        x: &[f64],
        perturbations: usize,
        draws: usize,
        with_radius: bool,
    ) -> Result<Metrics> {
        if x.len() != self.space.width() {
            return Err(Error::value("flaky: a coded case of the wrong width"));
        }
        let numeric = self.space.numeric_coords();
        let blocks = self.space.categorical_blocks();
        let config = self.config;
        // The same point gives the same metrics, whatever was measured before.
        let bits: String = x.iter().map(|v| format!("{:016x}", v.to_bits())).collect();
        let mut rng = rng_for(&format!("{}/{bits}", self.seed));
        let classifier = self.classifier()?;

        let mut points = vec![x.to_vec()];
        if !numeric.is_empty() {
            for _ in 0..perturbations {
                points.push(perturb(x, &numeric, config.epsilon, &mut rng));
            }
        }
        let (mean, cov) = classifier.latent(&points);
        let pass_probability = norm_cdf(mean[0] / (1.0 + cov[(0, 0)].max(0.0)).sqrt());
        let l = jittered_cholesky(cov);
        let m = points.len();
        let (mut flips, mut unstable) = (0.0, 0usize);
        for _ in 0..draws {
            let z = DVector::from_fn(m, |_, _| standard_normal(&mut rng));
            let f = &mean + &l * z;
            let p0 = norm_cdf(f[0]);
            let rate = if m == 1 {
                2.0 * p0 * (1.0 - p0)
            } else {
                (1..m)
                    .map(|k| {
                        let pk = norm_cdf(f[k]);
                        p0 * (1.0 - pk) + (1.0 - p0) * pk
                    })
                    .sum::<f64>()
                    / (m - 1) as f64
            };
            flips += rate;
            if rate > config.threshold {
                unstable += 1;
            }
        }
        let robustness_radius = if with_radius {
            radius(
                classifier,
                x,
                &numeric,
                &blocks,
                config.directions,
                &mut rng,
            )
        } else {
            None
        };
        Ok(Metrics {
            pass_probability,
            flip_probability: flips / draws as f64,
            instability_confidence: unstable as f64 / draws as f64,
            robustness_radius,
        })
    }

    /// The observations, most unstable first: `(index, metrics)`.
    pub fn ranking(&mut self) -> Result<Vec<(usize, Metrics)>> {
        let xs = self.x.clone();
        let mut out = Vec::with_capacity(xs.len());
        for (i, x) in xs.iter().enumerate() {
            out.push((i, self.metrics_at(x)?));
        }
        out.sort_by(|a, b| unstable_first(&a.1, &b.1).then(a.0.cmp(&b.0)));
        Ok(out)
    }
}

/// Most unstable first: by instability confidence, then flip probability.
fn unstable_first(a: &Metrics, b: &Metrics) -> std::cmp::Ordering {
    b.instability_confidence
        .total_cmp(&a.instability_confidence)
        .then(b.flip_probability.total_cmp(&a.flip_probability))
}

/// What an explored case counts for when the batch already has `n` cases with
/// its categories: `SAME_CATEGORIES_DECAY^n` times its instability. Without
/// it, a batch piles into the conditions whose flaky band is widest.
const SAME_CATEGORIES_DECAY: f64 = 0.7;

/// *x* moved along the numeric parameters onto the predicted boundary (a
/// latent mean of 0) by a few Newton steps, or `None` when that does not
/// bring it closer.
fn to_boundary(
    classifier: &Classifier,
    x: &[f64],
    numeric: &[(usize, &str, f64, f64)],
) -> Option<Vec<f64>> {
    const H: f64 = 1e-3;
    const MAX_STEP: f64 = 0.25;
    if numeric.is_empty() {
        return None;
    }
    let mut y = x.to_vec();
    let (mut start, mut end) = (None, f64::INFINITY);
    for step in 0..=4 {
        let mut points = vec![y.clone()];
        for (coord, ..) in numeric {
            for d in [H, -H] {
                let mut p = y.clone();
                p[*coord] += d;
                points.push(p);
            }
        }
        let m = classifier.latent_mean(&points);
        end = m[0].abs();
        start.get_or_insert(end);
        let grad: Vec<f64> = (0..numeric.len())
            .map(|i| (m[1 + 2 * i] - m[2 + 2 * i]) / (2.0 * H))
            .collect();
        let norm2: f64 = grad.iter().map(|g| g * g).sum();
        if step == 4 || norm2 < 1e-12 || end < 1e-3 {
            break;
        }
        let mut delta: Vec<f64> = grad.iter().map(|g| -m[0] * g / norm2).collect();
        let len = delta.iter().map(|d| d * d).sum::<f64>().sqrt();
        if len > MAX_STEP {
            delta.iter_mut().for_each(|d| *d *= MAX_STEP / len);
        }
        for ((coord, ..), d) in numeric.iter().zip(&delta) {
            y[*coord] = (y[*coord] + d).clamp(0.0, 1.0);
        }
    }
    start.is_some_and(|s| end < s).then_some(y)
}

/// A point uniform in the ball of radius *epsilon* about *x*, in the numeric
/// coordinates, kept in `[0, 1]`.
fn perturb(x: &[f64], numeric: &[usize], epsilon: f64, rng: &mut ChaCha8Rng) -> Vec<f64> {
    // Rejection: uniform over the part of the ball inside the cube. Clamping
    // instead would pile the points outside onto its faces -- at a corner,
    // most of them onto x itself.
    let mut last = x.to_vec();
    for _ in 0..64 {
        let direction: Vec<f64> = numeric.iter().map(|_| standard_normal(rng)).collect();
        let norm = direction
            .iter()
            .map(|d| d * d)
            .sum::<f64>()
            .sqrt()
            .max(1e-12);
        let radius = epsilon * rng.gen::<f64>().powf(1.0 / numeric.len() as f64);
        let mut out = x.to_vec();
        for (d, &i) in direction.iter().zip(numeric) {
            out[i] += radius * d / norm;
        }
        if numeric.iter().all(|&i| (0.0..=1.0).contains(&out[i])) {
            return out;
        }
        last = out;
    }
    // A case outside the cube, whose ball barely meets it: the last one, clamped.
    for &i in numeric {
        last[i] = last[i].clamp(0.0, 1.0);
    }
    last
}

/// The smallest distance at which the sign of the posterior mean changes.
fn radius(
    classifier: &Classifier,
    x: &[f64],
    numeric: &[usize],
    blocks: &[(usize, usize)],
    directions: usize,
    rng: &mut ChaCha8Rng,
) -> Option<f64> {
    let sign0 = classifier.latent_mean(&[x.to_vec()])[0];
    if sign0 == 0.0 {
        return Some(0.0);
    }
    let flipped = |v: f64| v.signum() != sign0.signum() || v == 0.0;
    let mut best: Option<f64> = None;
    let mut keep = |r: f64| best = Some(best.map_or(r, |b: f64| b.min(r)));

    // Along numeric directions: the axes both ways, then random ones.
    let mut dirs: Vec<Vec<f64>> = Vec::new();
    for k in 0..numeric.len() {
        for s in [1.0, -1.0] {
            let mut d = vec![0.0; numeric.len()];
            d[k] = s;
            dirs.push(d);
        }
    }
    if !numeric.is_empty() {
        for _ in 0..directions {
            let d: Vec<f64> = numeric.iter().map(|_| standard_normal(rng)).collect();
            let n = d.iter().map(|v| v * v).sum::<f64>().sqrt().max(1e-12);
            dirs.push(d.into_iter().map(|v| v / n).collect());
        }
    }
    let at = |d: &[f64], t: f64| {
        let mut p = x.to_vec();
        for (v, &i) in d.iter().zip(numeric) {
            p[i] += t * v;
        }
        p
    };
    const STEP: f64 = 0.01;
    for d in &dirs {
        // How far the ray stays in the unit cube.
        let reach = d
            .iter()
            .zip(numeric)
            .map(|(v, &i)| {
                if *v > 0.0 {
                    (1.0 - x[i]) / v
                } else if *v < 0.0 {
                    -x[i] / v
                } else {
                    f64::INFINITY
                }
            })
            .fold(f64::INFINITY, f64::min)
            .max(0.0);
        let steps = (reach / STEP).ceil() as usize;
        if steps == 0 {
            continue;
        }
        let ts: Vec<f64> = (1..=steps).map(|s| (s as f64 * STEP).min(reach)).collect();
        let points: Vec<Vec<f64>> = ts.iter().map(|t| at(d, *t)).collect();
        let means = classifier.latent_mean(&points);
        if let Some(k) = means.iter().position(|m| flipped(*m)) {
            let (mut lo, mut hi) = (if k == 0 { 0.0 } else { ts[k - 1] }, ts[k]);
            for _ in 0..30 {
                let mid = 0.5 * (lo + hi);
                if flipped(classifier.latent_mean(&[at(d, mid)])[0]) {
                    hi = mid;
                } else {
                    lo = mid;
                }
            }
            keep(hi);
        }
    }
    // Switching categories: each changed parameter is a step of 1, so a
    // combination changing k of them is √k away. Every combination when
    // there are few, else one parameter at a time.
    let combinations: usize = blocks
        .iter()
        .try_fold(1usize, |n, &(_, len)| n.checked_mul(len))
        .unwrap_or(usize::MAX);
    let mut switched: Vec<(Vec<f64>, f64)> = Vec::new();
    if combinations <= MAX_CATEGORY_COMBINATIONS {
        for code in 1..combinations {
            let (mut p, mut rest, mut changed) = (x.to_vec(), code, 0usize);
            for &(start, len) in blocks {
                let pick = rest % len;
                rest /= len;
                let current = (0..len).position(|i| x[start + i] > 0.0);
                if Some(pick) != current {
                    changed += 1;
                }
                for i in 0..len {
                    p[start + i] = if i == pick { CATEGORY_SCALE } else { 0.0 };
                }
            }
            if changed > 0 {
                switched.push((p, (changed as f64).sqrt()));
            }
        }
    } else {
        for &(start, len) in blocks {
            for other in 0..len {
                if x[start + other] > 0.0 {
                    continue;
                }
                let mut p = x.to_vec();
                for i in 0..len {
                    p[start + i] = if i == other { CATEGORY_SCALE } else { 0.0 };
                }
                switched.push((p, 1.0));
            }
        }
    }
    if !switched.is_empty() {
        let points: Vec<Vec<f64>> = switched.iter().map(|(p, _)| p.clone()).collect();
        for (mean, (_, distance)) in classifier.latent_mean(&points).iter().zip(&switched) {
            if flipped(*mean) {
                keep(*distance);
            }
        }
    }
    best
}

/// Above this many combinations of categories, the robustness radius
/// switches one categorical parameter at a time.
const MAX_CATEGORY_COMBINATIONS: usize = 1024;

// ---------------------------------------------------------------------------
// The search
// ---------------------------------------------------------------------------

/// Draw *count* cases. The sampler's own come first: as many as it still
/// needs to cover every bucket `coverage_depth` times
/// ([`Sampler::coverage_deficit`]), and at least all but `explore_fraction`
/// of the batch. The rest are the most unstable of `pool` candidates per
/// case. A model with no observations explores nothing. Returns each case with its
/// metrics when it was chosen for its instability.
pub fn search(
    sampler: &mut Sampler,
    model: &mut FlakinessModel,
    count: usize,
    explore_fraction: f64,
    pool: usize,
    coverage_depth: usize,
) -> Result<Vec<(Sample, Option<Metrics>)>> {
    if !(0.0..=1.0).contains(&explore_fraction) {
        return Err(Error::value("flaky: explore_fraction must be in [0, 1]"));
    }
    if model.space() != &ParamSpace::of(sampler) {
        return Err(Error::value(
            "flaky: the model's space is not the sampler's",
        ));
    }
    // The holes first: as many of the batch as the sampler still needs to
    // cover every bucket, then `explore_fraction` of the rest.
    let explore = if model.is_empty() {
        0
    } else {
        sampler.find_unreachable();
        let holes = sampler.coverage_deficit(coverage_depth).min(count);
        ((count as f64 * explore_fraction).round() as usize).min(count - holes)
    };
    let mut out: Vec<(Sample, Option<Metrics>)> = sampler
        .sample(count - explore)?
        .into_iter()
        .map(|s| (s, None))
        .collect();
    if explore == 0 {
        return Ok(out);
    }
    // Candidates from a copy: drawing them plans nothing in the sampler.
    let mut drawer = sampler.clone();
    let wanted = (explore * pool.max(1)).min(4096);
    let mut drawn: Vec<Sample> = Vec::new();
    let mut failures = 0;
    while drawn.len() < wanted && failures < wanted {
        match drawer.draw(0, None) {
            Ok(sample) => drawn.push(sample),
            Err(_) => failures += 1,
        }
    }
    // Each candidate also moved onto the predicted boundary, where the flaky
    // band is: drawn candidates seldom land in a narrow one.
    let moved: Vec<Sample> = {
        let space = model.space().clone();
        let numeric = space.numeric_dims();
        let xs: Vec<Vec<f64>> = drawn
            .iter()
            .map(|s| space.encode(&s.values))
            .collect::<Result<_>>()?;
        let classifier = model.classifier()?;
        drawn
            .iter()
            .zip(&xs)
            .filter_map(|(sample, x)| {
                let y = to_boundary(classifier, x, &numeric)?;
                let changes: Vec<(String, f64)> = numeric
                    .iter()
                    .map(|(coord, name, low, high)| {
                        (name.to_string(), low + y[*coord] * (high - low))
                    })
                    .collect();
                sampler.moved(sample, &changes)
            })
            .collect()
    };
    let (cheap_perturbations, cheap_draws) = (
        model.config.perturbations.min(16),
        model.config.draws.min(128),
    );
    let mut candidates: Vec<Candidate> = Vec::new();
    for sample in drawn.into_iter().chain(moved) {
        let x = model.space().encode(&sample.values)?;
        let metrics = model.measure(&x, cheap_perturbations, cheap_draws, false)?;
        let categories = model.space().categories(&x);
        candidates.push(Candidate {
            sample,
            x,
            metrics,
            categories,
        });
    }
    candidates.sort_by(|a, b| unstable_first(&a.metrics, &b.metrics));

    let mut taken: Vec<Vec<f64>> = out
        .iter()
        .map(|(s, _)| model.space().encode(&s.values))
        .collect::<Result<_>>()?;
    let epsilon = model.config.epsilon;
    let far = |x: &[f64], taken: &[Vec<f64>]| {
        taken.iter().all(|t| {
            t.iter()
                .zip(x)
                .map(|(a, b)| (a - b) * (a - b))
                .sum::<f64>()
                .sqrt()
                >= epsilon
        })
    };
    let mut per_categories: HashMap<&[Option<usize>], i32> = HashMap::new();
    let mut used = vec![false; candidates.len()];
    let mut chosen: Vec<usize> = Vec::new();
    while chosen.len() < explore {
        // The most unstable apart from those taken, a combination of
        // categories already in the batch counting for less each time; the
        // candidates are sorted, so a tie goes to the earlier.
        let best = candidates
            .iter()
            .enumerate()
            .filter(|(i, c)| !used[*i] && far(&c.x, &taken))
            .map(|(i, c)| {
                let weight = SAME_CATEGORIES_DECAY
                    .powi(*per_categories.get(c.categories.as_slice()).unwrap_or(&0));
                let scaled = Metrics {
                    instability_confidence: c.metrics.instability_confidence * weight,
                    flip_probability: c.metrics.flip_probability * weight,
                    ..c.metrics
                };
                (i, scaled)
            })
            .min_by(|a, b| unstable_first(&a.1, &b.1).then(a.0.cmp(&b.0)));
        let Some((i, _)) = best else { break };
        used[i] = true;
        chosen.push(i);
        taken.push(candidates[i].x.clone());
        *per_categories
            .entry(candidates[i].categories.as_slice())
            .or_default() += 1;
    }
    // Not enough apart: the next most unstable, close or not.
    let missing = explore - chosen.len();
    chosen.extend((0..candidates.len()).filter(|i| !used[*i]).take(missing));

    let base = out.len();
    for (j, i) in chosen.into_iter().enumerate() {
        let Candidate { mut sample, x, .. } = candidates[i].clone();
        sample.index = base + j;
        let metrics = model.metrics_at(&x)?;
        sampler.plan(&sample);
        out.push((sample, Some(metrics)));
    }
    Ok(out)
}

/// A case the search may explore: coded, scored, and its categories.
#[derive(Clone)]
struct Candidate {
    sample: Sample,
    x: Vec<f64>,
    metrics: Metrics,
    categories: Vec<Option<usize>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(pairs: &[(&str, Value)]) -> IndexMap<String, Value> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    fn space() -> ParamSpace {
        ParamSpace::new(vec![
            Dim::Numeric {
                name: "gap".into(),
                low: 0.0,
                high: 100.0,
            },
            Dim::Categorical {
                name: "rain".into(),
                labels: vec!["none".into(), "heavy".into()],
            },
        ])
    }

    /// Runs on a grid of gap: pass above 50 m, fail below.
    fn model() -> FlakinessModel {
        let mut m = FlakinessModel::new(space(), Config::default(), "0").unwrap();
        for g in (0..=100).step_by(5) {
            let passed = g > 50;
            m.observe(
                &values(&[
                    ("gap", Value::Float(g as f64)),
                    ("rain", Value::Str("none".into())),
                ]),
                passed,
            )
            .unwrap();
        }
        m
    }

    #[test]
    fn the_normal_distribution_is_right() {
        assert!((norm_cdf(0.0) - 0.5).abs() < 1e-7);
        assert!((norm_cdf(1.959_964) - 0.975).abs() < 1e-6);
        assert!((norm_cdf(-1.959_964) - 0.025).abs() < 1e-6);
        // The inverse Mills ratio is continuous where the tail form takes over.
        assert!((inverse_mills(-6.0 + 1e-9) - inverse_mills(-6.0 - 1e-9)).abs() < 1e-3);
        assert!(log_cdf(-40.0).is_finite());
    }

    #[test]
    fn a_space_codes_numbers_and_categories() {
        let s = space();
        let x = s
            .encode(&values(&[
                ("gap", Value::Float(25.0)),
                ("rain", Value::Str("heavy".into())),
            ]))
            .unwrap();
        assert_eq!(x, vec![0.25, 0.0, CATEGORY_SCALE]);
        assert!(s.encode(&values(&[("gap", Value::Float(1.0))])).is_err());
        assert!(s
            .encode(&values(&[
                ("gap", Value::Float(1.0)),
                ("rain", Value::Str("snow".into()))
            ]))
            .is_err());
    }

    #[test]
    fn the_classifier_learns_the_boundary() {
        let mut m = model();
        let c = m.classifier().unwrap().clone();
        let at = |g: f64| vec![g / 100.0, CATEGORY_SCALE, 0.0];
        assert!(c.pass_probability(&at(95.0)) > 0.8);
        assert!(c.pass_probability(&at(5.0)) < 0.2);
    }

    #[test]
    fn the_boundary_is_flakier_than_the_far_side() {
        let mut m = model();
        let near = m
            .metrics(&values(&[
                ("gap", Value::Float(52.0)),
                ("rain", Value::Str("none".into())),
            ]))
            .unwrap();
        let far = m
            .metrics(&values(&[
                ("gap", Value::Float(95.0)),
                ("rain", Value::Str("none".into())),
            ]))
            .unwrap();
        assert!(
            near.flip_probability > far.flip_probability,
            "{near:?} {far:?}"
        );
        assert!(near.instability_confidence >= far.instability_confidence);
        let (rn, rf) = (
            near.robustness_radius.unwrap(),
            far.robustness_radius.unwrap(),
        );
        assert!(rn < rf, "{rn} {rf}");
        // The boundary is about 0.45 below 95 m in the normalized space.
        assert!((rf - 0.43).abs() < 0.1, "{rf}");
    }

    #[test]
    fn metrics_do_not_depend_on_what_was_measured_before() {
        let mut a = model();
        let mut b = model();
        let v = values(&[
            ("gap", Value::Float(40.0)),
            ("rain", Value::Str("none".into())),
        ]);
        let _ = b
            .metrics(&values(&[
                ("gap", Value::Float(10.0)),
                ("rain", Value::Str("none".into())),
            ]))
            .unwrap();
        assert_eq!(a.metrics(&v).unwrap(), b.metrics(&v).unwrap());
    }

    #[test]
    fn a_coin_toss_is_flaky_where_it_stands() {
        let mut m = FlakinessModel::new(space(), Config::default(), "0").unwrap();
        let v = values(&[
            ("gap", Value::Float(30.0)),
            ("rain", Value::Str("none".into())),
        ]);
        for i in 0..20 {
            m.observe(&v, i % 2 == 0).unwrap();
        }
        let metrics = m.metrics(&v).unwrap();
        assert!((metrics.pass_probability - 0.5).abs() < 0.1, "{metrics:?}");
        assert!(metrics.flip_probability > 0.3, "{metrics:?}");
        assert!(metrics.instability_confidence > 0.9, "{metrics:?}");
    }

    #[test]
    fn a_fixed_parameter_is_never_perturbed() {
        let s = ParamSpace::new(vec![
            Dim::Numeric {
                name: "fixed".into(),
                low: 5.0,
                high: 5.0,
            },
            Dim::Numeric {
                name: "x".into(),
                low: 0.0,
                high: 1.0,
            },
        ]);
        assert_eq!(s.numeric_coords(), vec![1]);
    }

    #[test]
    fn perturbations_stay_in_the_cube_without_piling_on_its_face() {
        let mut rng = rng_for("p");
        let x = vec![0.0];
        let points: Vec<Vec<f64>> = (0..200)
            .map(|_| perturb(&x, &[0], 0.05, &mut rng))
            .collect();
        assert!(points.iter().all(|p| (0.0..=0.05).contains(&p[0])));
        // Uniform over [0, 0.05]: none sits exactly on the face.
        assert!(points.iter().all(|p| p[0] > 0.0));
    }

    #[test]
    fn a_combination_of_categories_can_be_the_nearest_change() {
        // Two binary parameters: only "11" passes.
        let s = ParamSpace::new(vec![
            Dim::Categorical {
                name: "a".into(),
                labels: vec!["0".into(), "1".into()],
            },
            Dim::Categorical {
                name: "b".into(),
                labels: vec!["0".into(), "1".into()],
            },
        ]);
        let mut m = FlakinessModel::new(s, Config::default(), "0").unwrap();
        for _ in 0..6 {
            for (a, b) in [("0", "0"), ("0", "1"), ("1", "0"), ("1", "1")] {
                let v = values(&[("a", Value::Str(a.into())), ("b", Value::Str(b.into()))]);
                m.observe(&v, a == "1" && b == "1").unwrap();
            }
        }
        let r = m
            .metrics(&values(&[
                ("a", Value::Str("0".into())),
                ("b", Value::Str("0".into())),
            ]))
            .unwrap()
            .robustness_radius;
        assert_eq!(r, Some(std::f64::consts::SQRT_2));
    }

    #[test]
    fn a_category_is_never_called_entirely_irrelevant() {
        // Rain changes nothing here, yet its length scale stays within the
        // prior's range: runs in one category still inform another.
        let mut m = model();
        let c = m.classifier().unwrap();
        assert!(c.length_scales[1] <= LengthScalePrior::CATEGORICAL.max);
        assert!(c.length_scales[0] <= LengthScalePrior::NUMERIC.max);
        let prior = LengthScalePrior::CATEGORICAL;
        assert!(prior.log_density(prior.median) > prior.log_density(prior.max));
    }

    #[test]
    fn the_ranking_puts_the_boundary_first() {
        let mut m = model();
        let ranking = m.ranking().unwrap();
        let top: f64 = ranking[0].0 as f64 * 5.0; // gap of the first observation
        assert!((top - 50.0).abs() <= 10.0, "{top}");
    }
}
