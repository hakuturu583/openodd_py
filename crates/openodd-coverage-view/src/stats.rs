//! How sure the runs of a region are that it passes, fails, or both.
//!
//! A region (an ODD cell, or any union of them) visited by `runs` runs with
//! an outcome, `failed` of which failed, has a failure rate with a 95% Wilson
//! score interval, and a [`CellStatus`] from two explicit thresholds: the
//! fewest runs to judge it (`min_runs`) and the band the interval must lie
//! inside for it to be called flaky (`band`). `openodd`'s per-cell report and
//! this crate's matrix share these, so they never disagree.

/// The z of a two-sided 95% interval.
pub const Z95: f64 = 1.959_963_984_540_054;
/// The fewest runs that must have visited a cell before it is called flaky
/// (or always failing), unless asked otherwise.
pub const MIN_RUNS: u64 = 3;
/// A cell is flaky when its interval lies inside this band, unless asked
/// otherwise: both outcomes then happen at least 5% of the time, at 95%
/// confidence.
pub const BAND: (f64, f64) = (0.05, 0.95);

/// The Wilson score interval of `failed` failures in `runs` runs, at `z`;
/// `(0, 1)` when there are no runs.
pub fn wilson(failed: u64, runs: u64, z: f64) -> (f64, f64) {
    if runs == 0 {
        return (0.0, 1.0);
    }
    let n = runs as f64;
    let p = failed as f64 / n;
    let z2 = z * z;
    let denom = 1.0 + z2 / n;
    let centre = (p + z2 / (2.0 * n)) / denom;
    let half = z * (p * (1.0 - p) / n + z2 / (4.0 * n * n)).sqrt() / denom;
    // Exactly 0 (1) with no failure (no pass), whatever the rounding.
    let low = if failed == 0 {
        0.0
    } else {
        (centre - half).max(0.0)
    };
    let high = if failed >= runs {
        1.0
    } else {
        (centre + half).min(1.0)
    };
    (low, high)
}

/// What a cell's runs say of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellStatus {
    /// Both outcomes, and the interval inside the band.
    Flaky,
    /// Every run that visited it failed: a bug, not flakiness.
    Failing,
    /// Both outcomes, but too few to tell from a rare one (the interval
    /// reaches outside the band).
    Mixed,
    /// No run that visited it failed.
    Passing,
    /// Fewer runs than the minimum visited it.
    TooFew,
}

impl CellStatus {
    /// Where its cells sort: what most needs a look first.
    pub fn rank(&self) -> u8 {
        match self {
            CellStatus::Flaky => 0,
            CellStatus::Failing => 1,
            CellStatus::Mixed => 2,
            CellStatus::TooFew => 3,
            CellStatus::Passing => 4,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            CellStatus::Flaky => "flaky",
            CellStatus::Failing => "failing",
            CellStatus::Mixed => "mixed",
            CellStatus::Passing => "passing",
            CellStatus::TooFew => "too_few",
        }
    }
}

/// The status of a cell visited by `runs` runs, `failed` of them failed.
pub fn status(runs: u64, failed: u64, min_runs: u64, band: (f64, f64)) -> CellStatus {
    if runs == 0 || runs < min_runs {
        return CellStatus::TooFew;
    }
    if failed == 0 {
        return CellStatus::Passing;
    }
    if failed >= runs {
        return CellStatus::Failing;
    }
    let (low, high) = wilson(failed, runs, Z95);
    if band.0 < low && high < band.1 {
        CellStatus::Flaky
    } else {
        CellStatus::Mixed
    }
}
