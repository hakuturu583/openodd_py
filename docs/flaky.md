# Finding flaky cases

A test that passes at one setting and fails a hair away — or both passes and fails at
the same setting — is *flaky*. Coverage does not find it: the sampler spreads runs over
the ODD's buckets, while instability sits on the boundary between passing and failing.
`FlakySearch` learns that boundary from the runs so far and spends part of each batch on
it. The sampler's own policy (uniform, or the least covered buckets first) still draws
the rest.

```python
import openodd

sampler = openodd.OddSampler(odd, controls=controls, strategy="coverage", seed=0)
search = openodd.FlakySearch(sampler, explore_fraction=0.5)

for case, passed in earlier_runs:          # an OddSample (or its values) and the outcome
    search.observe(case, passed)

for case in search.sample(20):             # the first 10 the sampler's, the rest flaky
    run(case.overrides)
    if case.flaky is not None:             # chosen for its instability
        print(case.flaky.flip_probability, case.flaky.robustness_radius)

for values, passed, metrics in search.ranking()[:5]:   # the runs so far, flakiest first
    print(values, passed, metrics.instability_confidence)
```

## The parameters, normalized

A case is coded in a normalized space (`ParameterSpace`), so that parameters of any unit
or kind can be measured against each other:

| Parameter | Coded as |
|---|---|
| numeric, over `[low, high]` | `(value - low) / (high - low)`: `[0, 1]` over its range |
| categorical, one of `labels` | one-hot, scaled by `1/√2`: two categories are 1 apart |

`FlakySearch` takes the space from the sampler. It covers the attributes the sampler
draws: a numeric one over what its knob can bring about in its drawable buckets, and a
categorical one over its drawable buckets. `FlakinessModel` takes any space, so runs
described by parameters of your own can be modelled too:

```python
space = openodd.ParameterSpace(
    numeric={"speed_kph": (0, 120), "friction": (0.2, 1.0)},
    categorical={"lane": [1, 2, 3]},
)
model = openodd.FlakinessModel(space, observations)
model.metrics({"speed_kph": 76, "friction": 0.6, "lane": 1})
```

Distances below (`epsilon`, `robustness_radius`) are in this space.

## The model

Pass/fail is a binary classification problem. A latent function `f ~ GP(0, k)` gives
`P(pass | f) = Φ(f)` (a probit likelihood). Its posterior, given the runs, is the Laplace
approximation (Rasmussen & Williams, *Gaussian Processes for Machine Learning*,
algorithms 3.1 and 3.2).

The kernel `k` is squared-exponential, with a length scale per parameter (automatic
relevance determination). A parameter the outcome does not depend on ends up with a long
length scale, so runs that differ only in it inform each other. The length scales start
from the one fitted by a GP regression on the ±1 outcomes, using the
[`friedrich`](https://crates.io/crates/friedrich) crate (marginal likelihood, gradient
descent). A categorical parameter starts from its prior's median instead. Each
parameter's length scale, and the amplitude, are then refined in turn by the Laplace
approximation's evidence plus a log-normal prior on each length scale (a MAP estimate):

| Parameter | Prior median | log-sd | Range |
|---|---|---|---|
| numeric | 0.3 (about a third of its range) | 1 | 0.01 – 10 |
| categorical | 1 (two categories correlated by e^-1/2) | 1 | 0.05 – 4 |

The prior matters with few runs. The evidence alone can call a parameter irrelevant (a
length scale of 100) when it only matters near the boundary, where the runs are not yet:
in a closed-loop check on a cut-in ODD (160 runs), it did so for the rain, whose friction
and noise move the boundary. A categorical length scale is kept below 4 (a correlation
of 0.97 between categories), so the model may find a category nearly irrelevant, never
entirely. The amplitude is capped at 16 (a latent standard
deviation of 4): on separable outcomes the evidence keeps rising with the amplitude, and
a huge prior variance would pull every prediction far from the runs back to a coin toss.
`FlakinessModel.kernel()` shows what was fitted.

## The metrics

For a case `x` (`FlakyMetrics`):

| Metric | What it is |
|---|---|
| `pass_probability` | `P(pass)` at `x`, the latent function integrated out: `Φ(μ / √(1 + σ²))` |
| `flip_probability` | The estimated chance that a run at `x` and one at a small change `x'` of it disagree, averaged over the posterior |
| `instability_confidence` | The posterior probability that that flip rate exceeds `threshold` |
| `robustness_radius` | The smallest distance from `x` at which the predicted outcome changes; `None` when nothing in the space changes it |

**How they are computed:**

- **Small changes**: `perturbations` points `x'` (32 by default) are drawn uniformly in
  the part of the ball of radius `epsilon` (0.05) about `x` that lies inside the space
  (by rejection, so a case at an edge is not biased towards itself). Only the numeric
  parameters change; one fixed at a single value (`low == high`) never does.
- **Flip rate per draw**: `draws` joint samples (256) are taken of `f` at `x` and its
  neighbours from the posterior. For each sample, with `p = Φ(f(x))` and
  `p' = Φ(f(x'))`, its flip rate is the mean of `p (1 - p') + (1 - p) p'` over the
  neighbours.
- **`flip_probability`** is the mean of those flip rates.
- **`instability_confidence`** is the share of draws whose flip rate is above
  `threshold` (0.1).
- **A coin toss is flaky too**: a setting that both passed and failed has `p ≈ 0.5`, and
  so a flip rate near 0.5 even with no change at all.
- **`robustness_radius`**: the sign of the posterior mean of `f` (the predicted outcome)
  is followed out of `x` in steps of 0.01. The search goes along each numeric axis both
  ways and along `directions` (16) random directions, to the edge of the space, and the
  first change is refined by bisection. Switching `k` categorical parameters counts as a
  step of `√k`. Every combination of categories is tried while there are at most 1024;
  beyond that, one categorical parameter at a time. The radius is the smallest change
  the search finds, so it is an estimate from above.

The same seed, runs and case always give the same metrics, whatever was measured before.

## The search

`FlakySearch.sample(count)` draws `round(count × explore_fraction)` cases for
instability and the rest with the sampler's own policy, the holes first:

1. **The sampler's own cases**: drawn first, exactly as `OddSampler.sample` would draw
   them. While the coverage strategy has buckets covered fewer than `coverage_depth`
   times (1), the cases it needs to cover them are its own whatever `explore_fraction`
   says: a case covers one bucket of each attribute, so it needs as many as the
   attribute lacking most lacks. Once the buckets are covered, `explore_fraction` of
   each batch is explored. `coverage_depth=0` explores from the first observed run; the
   uniform strategy has no holes to fill.
2. **Candidates**: `pool` candidates per explored case (16) are drawn from the ODD by a
   copy of the sampler. They are combinations the ODD admits, set by the same knobs, and
   drawing them plans nothing in the sampler itself.
3. **Ranking**: each candidate is scored with fewer perturbations and draws, then ranked
   by `instability_confidence`, then `flip_probability`.
4. **Spacing**: the most unstable candidates are kept, each at least `epsilon` from the
   others and from the sampler's cases (closer ones only when too few are apart). Their
   full metrics are in `OddSample.flaky`, and the sampler counts them as planned, so its
   coverage strategy goes on from them.

Before any run is observed there is nothing to learn from, so the whole batch is the
sampler's.

## Cost

Fitting is `O(n³)` in the runs observed, repeated for each step of the length-scale
refinement (three passes up to 400 runs, one beyond). In a release build it takes about
0.03 s at 40 runs, 0.5 s at 200 and 5 s at 400. A batch of 20 then takes well under a
second. The model is fitted again only after new runs are observed.
