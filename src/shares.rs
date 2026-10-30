//! The arithmetic of the weighted strategy: how many of a batch go where.
//!
//! The weighted strategy is given a share per module -- `heavy_rain: 0.3`,
//! "30% of the cases in module `heavy_rain`" -- and draws a batch whose
//! fraction in each module is that share. Modules overlap, so the cases fall
//! into *cells*: one per pattern of membership (in `heavy_rain`, not in
//! `night`, ...). The sampler estimates each cell's mass under the uniform
//! strategy from a pool of its cases; [`rake`] scales the masses until every
//! module holds its share (iterative proportional fitting: of all the
//! distributions that do, the one closest to uniform, so inside a cell the
//! cases are as the uniform strategy draws them), and [`allocate`] turns the
//! masses into whole cases.

use crate::error::{Error, Result};

/// The sweeps [`rake`] makes before it gives up.
const SWEEPS: usize = 10_000;
/// How far a module's mass may end from its share.
const TOLERANCE: f64 = 1e-9;

/// The masses of `cells` (each a pattern of membership, one flag per module)
/// scaled so that the cells in module `j` hold `shares[j]` together. `mass`
/// is what each cell holds before, summing to 1. `names` are the modules,
/// for messages.
pub fn rake(
    cells: &[Vec<bool>],
    mass: &[f64],
    shares: &[f64],
    names: &[String],
) -> Result<Vec<f64>> {
    let total: f64 = mass.iter().sum();
    let mut m: Vec<f64> = mass.iter().map(|x| x / total).collect();
    for (j, share) in shares.iter().enumerate() {
        let some = |inside: bool| {
            cells
                .iter()
                .zip(&m)
                .any(|(c, x)| c[j] == inside && *x > 0.0)
        };
        if *share > 0.0 && !some(true) {
            return Err(Error::value(format!(
                "OddSampler: no case drawn inside the ODD is in module {}, so it cannot hold {}",
                names[j],
                percent(*share)
            )));
        }
        if *share < 1.0 && !some(false) {
            return Err(Error::value(format!(
                "OddSampler: every case drawn inside the ODD is in module {}, so it cannot hold only {}",
                names[j],
                percent(*share)
            )));
        }
    }
    for _ in 0..SWEEPS {
        for (j, share) in shares.iter().enumerate() {
            let inside = held(cells, &m, j);
            let (up, down) = (share / inside, (1.0 - share) / (1.0 - inside));
            for (cell, x) in cells.iter().zip(m.iter_mut()) {
                *x *= if cell[j] { up } else { down };
                if !x.is_finite() {
                    *x = 0.0;
                }
            }
        }
        let worst = (0..shares.len())
            .map(|j| (held(cells, &m, j) - shares[j]).abs())
            .fold(0.0, f64::max);
        if worst < TOLERANCE {
            return Ok(m);
        }
    }
    let reached: Vec<String> = names
        .iter()
        .enumerate()
        .map(|(j, n)| {
            format!(
                "{n} {} of {}",
                percent(held(cells, &m, j)),
                percent(shares[j])
            )
        })
        .collect();
    Err(Error::value(format!(
        "OddSampler: the shares cannot all hold at once inside the ODD (closest: {})",
        reached.join(", ")
    )))
}

/// What the cells in module `j` hold together.
fn held(cells: &[Vec<bool>], mass: &[f64], j: usize) -> f64 {
    cells
        .iter()
        .zip(mass)
        .filter(|(c, _)| c[j])
        .map(|(_, x)| x)
        .sum()
}

fn percent(x: f64) -> String {
    format!("{}%", (x * 1000.0).round() / 10.0)
}

/// `total` whole cases split in proportion to `weights` (largest remainder;
/// a tie goes to the earlier). The parts sum to `total`; weights all 0 give
/// nothing.
pub fn apportion(total: usize, weights: &[f64]) -> Vec<usize> {
    let sum: f64 = weights.iter().sum();
    if sum <= 0.0 || total == 0 {
        return vec![0; weights.len()];
    }
    let exact: Vec<f64> = weights.iter().map(|w| w / sum * total as f64).collect();
    let mut parts: Vec<usize> = exact.iter().map(|x| x.floor() as usize).collect();
    let mut order: Vec<usize> = (0..weights.len()).collect();
    order.sort_by(|a, b| {
        let (ra, rb) = (exact[*a] - parts[*a] as f64, exact[*b] - parts[*b] as f64);
        rb.total_cmp(&ra).then(a.cmp(b))
    });
    let given: usize = parts.iter().sum();
    for i in order.into_iter().take(total.saturating_sub(given)) {
        parts[i] += 1;
    }
    parts
}

/// How many of `count` new cases each cell gets, so that the cells hold
/// `mass` of the new cases together with `have` (the cases drawn before,
/// per cell) as closely as whole cases allow: what each cell lacks of its
/// part of all of them, the cases split in proportion where the cells lack
/// more than `count`.
pub fn allocate(count: usize, mass: &[f64], have: &[usize]) -> Vec<usize> {
    let all = count + have.iter().sum::<usize>();
    let wanted = apportion(all, mass);
    let lacking: Vec<f64> = wanted
        .iter()
        .zip(have)
        .map(|(w, h)| w.saturating_sub(*h) as f64)
        .collect();
    if lacking.iter().sum::<f64>() > 0.0 {
        apportion(count, &lacking)
    } else {
        apportion(count, mass)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("m{i}")).collect()
    }

    #[test]
    fn one_module_gets_its_share_and_the_rest_keep_their_proportions() {
        let cells = vec![vec![true], vec![false], vec![false]];
        let m = rake(&cells, &[0.1, 0.3, 0.6], &[0.5], &names(1)).unwrap();
        assert!((m[0] - 0.5).abs() < 1e-9);
        assert!((m[1] - 0.5 / 3.0).abs() < 1e-9 && (m[2] - 1.0 / 3.0).abs() < 1e-9);
    }

    #[test]
    fn overlapping_modules_each_hold_their_share() {
        // in a, in b: both, a only, b only, neither; uniform before.
        let cells = vec![
            vec![true, true],
            vec![true, false],
            vec![false, true],
            vec![false, false],
        ];
        let m = rake(&cells, &[0.25; 4], &[0.3, 0.6], &names(2)).unwrap();
        assert!((m[0] + m[1] - 0.3).abs() < 1e-9);
        assert!((m[0] + m[2] - 0.6).abs() < 1e-9);
        // Independent before, independent after.
        assert!((m[0] - 0.18).abs() < 1e-9);
    }

    #[test]
    fn shares_that_cannot_hold_together_are_refused() {
        // a and b never overlap: 0.7 + 0.6 does not fit.
        let cells = vec![vec![true, false], vec![false, true], vec![false, false]];
        let e = rake(&cells, &[0.2, 0.2, 0.6], &[0.7, 0.6], &names(2)).unwrap_err();
        assert!(e.to_string().contains("cannot all hold at once"), "{e}");
        let e = rake(&cells, &[0.0, 0.5, 0.5], &[0.1, 0.5], &names(2)).unwrap_err();
        assert!(e.to_string().contains("no case drawn"), "{e}");
    }

    #[test]
    fn apportion_sums_to_the_total() {
        assert_eq!(apportion(10, &[0.3, 0.7]), [3, 7]);
        assert_eq!(apportion(1, &[0.3, 0.7]), [0, 1]);
        assert_eq!(apportion(3, &[1.0, 1.0, 1.0]), [1, 1, 1]);
        assert_eq!(apportion(2, &[1.0, 1.0, 1.0]), [1, 1, 0]);
        assert_eq!(apportion(5, &[0.0, 0.0]), [0, 0]);
    }

    #[test]
    fn a_batch_of_small_draws_adds_up_to_the_shares() {
        // One case at a time, each draw knowing the earlier: 30% / 70% overall.
        let mass = [0.3, 0.7];
        let mut have = vec![0, 0];
        for _ in 0..10 {
            let got = allocate(1, &mass, &have);
            have[0] += got[0];
            have[1] += got[1];
        }
        assert_eq!(have, [3, 7]);
    }
}
