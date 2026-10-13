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
search = openodd.FlakySearch(sampler)      # 80% of each batch explored once covered

for case, passed in earlier_runs:          # an OddSample (or its values) and the outcome
    search.observe(case, passed)

for case in search.sample(20):             # the first 4 the sampler's, the rest flaky
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
   attribute lacking most lacks. Once the buckets are covered, `explore_fraction` (0.8)
   of each batch is explored; the rest keeps sweeping the ODD. `coverage_depth=0` explores from the first observed run; the
   uniform strategy has no holes to fill.

   A bucket the knobs can set may still be in no case the ODD admits: heavy rain
   ruled out by day, on a simulator that cannot make the night. On its first draw, a
   sampler with the coverage strategy tries each bucket once (a case drawn with that
   bucket, on a copy of the sampler); one that cannot be drawn is given up on with a
   warning, listed in `OddSampler.unreachable`, and is neither counted nor aimed at. It shows where the ODD asks for more than the
   test bench can bring about.
2. **Candidates**: `pool` candidates per explored case (16) are drawn from the ODD by a
   copy of the sampler. They are combinations the ODD admits, set by the same knobs, and
   drawing them plans nothing in the sampler itself.
3. **Onto the boundary**: each candidate is also moved, along its numeric parameters
   only, onto the predicted boundary (a latent mean of 0) by a few Newton steps. Where
   the outcome barely depends on chance, the flaky band is narrow and drawn candidates
   seldom land in it. A moved case is kept as a candidate when the knobs can set its
   values and the ODD admits it.
4. **Ranking**: each candidate is scored with fewer perturbations and draws, by
   `instability_confidence`, then `flip_probability`.
5. **Picking**: the most unstable candidates are taken one at a time, each at least
   `epsilon` from the others and from the sampler's cases (closer ones only when too
   few are apart). A candidate whose categories are already in the batch `n` times
   counts for `0.7ⁿ` of its instability, so the batch is not piled into the
   conditions where the band is widest. Their
   full metrics are in `OddSample.flaky`, and the sampler counts them as planned, so its
   coverage strategy goes on from them.

Before any run is observed there is nothing to learn from, so the whole batch is the
sampler's.

## Recording the runs

What the search computes is written into each run's coverage file, so that the
report can be read back after the campaign and the next campaign can start from it:

```python
for case in search.sample(20):
    collector = openodd.CoverageCollector(items, odd=odd)
    collector.record_case(case, space=search.space)   # values, why chosen, predicted flakiness
    passed = run(case.overrides, collector)
    collector.record_outcome(passed)   # optionally failed_at=, environment= (below)
    collector.write(out / f"{name}_coverage.json", name)
    search.observe(case, passed)
```

The coverage file gains two entries, both `null` when nothing was recorded:

```json
"case": {
  "index": 3, "situation": null,
  "values": {"environment.rain": "heavy", "dynamic.vehicle_ahead_gap": 21.4},
  "buckets": {...}, "overrides": [...],
  "chosen_for": "instability",
  "flaky": {"pass_probability": 0.48, "flip_probability": 0.31,
            "instability_confidence": 0.94, "robustness_radius": 0.03},
  "space": [{"name": "environment.rain", "labels": ["none", "light", "heavy"]},
            {"name": "dynamic.vehicle_ahead_gap", "range": [5.0, 60.0]}]
},
"outcome": {"passed": false}
```

`chosen_for` is `instability` for a case the search chose (its `flaky` is the
prediction made then) and `coverage` for one the sampler's own policy drew.

**The merged report** (`merge_coverage`, `openodd-coverage`) keeps every such run
(`CoverageReport.cases`, a `CaseRun` each). It refits the flakiness of each run over
all the runs of its scenario that were chosen in the same space and recorded an
outcome (`CaseRun.assessed`), with the model's defaults. Its Markdown gains a
**Flaky cases** section: how many runs were chosen for instability and by the
sampler's policy, how many of each failed, how many are flaky as assessed
(`instability_confidence` above 0.5), and the runs flakiest first, with their
predicted and assessed flakiness side by side (at most 20 rows).

**The next campaign** starts from them:

```python
report = openodd.load_and_merge([previous_results])
search = openodd.FlakySearch(sampler)
search.observe_report(report, scenario="cut_in")   # the runs chosen in this space
```

Only runs with an outcome, chosen in the same space (the same parameters, ranges
and labels), are observed.

## Flakiness per ODD cell

The flakiness above is per *case*: a model over the parameters a run was chosen
with, asking "would a small change of these values flip the outcome?". The merged
report also answers a different question, with no model at all: **in which regions
of the ODD do runs fail?** A *cell* is one bucket of each of the ODD's bucketed
attributes (`openodd.OddCells`); every coverage file already says how long its run
spent in each cell (`odd.cells`). With the run's outcome, each cell gets:

| Field | Meaning |
|---|---|
| `runs_visited` | Runs with an outcome that spent any step in the cell |
| `runs_failed` | Of those, how many failed |
| `fail_rate` | `runs_failed / runs_visited` |
| `ci_low`, `ci_high` | Its 95% Wilson score interval |
| `fail_at_cell` | Failed runs that were *in this cell* when they failed |
| `seconds`, `meters` | Time and distance the runs spent in the cell |

A cell no run visited has no row: its rate is undefined. A run of no steps counts
in its group's `runs` but visits no cell. A `<missing>`, `<below>` or `<above>`
label is a cell like any other.

### Recording where and on what a run failed

```python
collector.record_outcome(
    passed,
    failed_at=elapsed_at_failure,      # seconds, on the clock given to tick()
    environment=environment_id,        # e.g. sha256 of the lock file
)
```

Both are optional and written into `outcome` only when given; a file written
before them (`{"passed": false}`), or with no outcome at all, merges exactly as
before. `failed_at` is on the clock the collector is given (the `elapsed` of
`start()` and `tick()`, the first column of `odd.samples.rows`), which is seconds
since `odd.started_at` when that clock starts at zero. The collector then also
writes the cell of the last tick at or before it:

```json
"outcome": {"passed": false, "failed_at": 7.236,
            "failed_cell": ["[30, 60]", "heavy"], "environment": "sha256:3f9a..."}
```

It is written by the collector, not worked out in the merge, because a coverage
file does not carry the attributes' bucketing rules -- only their labels -- so the
merge could not bucket a sample row again without guessing. A failure before the
first tick, a run without an ODD, a tick whose cell could not be counted, or a
file from another writer without `failed_cell` leaves the failure *unlocated*: the
run still counts in `runs_failed` of every cell it visited, and in no cell's
`fail_at_cell` (the group's `failures_located` says how many were located). A
`failed_at` after the last tick takes the last tick's cell. `failed_at` on a run
that passed is ignored with a warning.

### Groups: scenario, environment, attributes

Runs are pooled only with runs of the **same scenario** and the **same
environment**. The environment is an opaque ID of what the run ran on -- the
software under test and its dependencies. Pooling runs of two versions turns a
regression into "flakiness": a cell that always passes on one build and always
fails on the next would read as a 50% flaky cell. Runs that recorded no
environment form a group of their own. Within a group, only runs whose ODD has the
**same attribute list** are pooled -- cells of different lists are different
things; runs of one scenario and environment with different lists are reported
apart, with a warning naming the documents of each list. (Different bucket
edges show up as different labels, so they are never confused either.)

### Reading it: the interval, the status, `fail_at_cell`

The interval is the **Wilson score interval** at 95%. It is closed-form (no Beta
quantile, nothing to add to the Rust core), stays inside [0, 1], is not
degenerate at 0 or n failures (unlike the Wald interval), and covers close to its
nominal 95% for the handful of runs a cell usually has. Jeffreys' interval would
do as well but needs the inverse incomplete Beta function.

Each cell has a status, from two explicit parameters, `min_runs` (default
`openodd.CELL_MIN_RUNS` = 3) and `band` (default `openodd.CELL_BAND` =
(0.05, 0.95)):

| Status | When |
|---|---|
| `flaky` | at least `min_runs` runs, both outcomes, and the interval lies inside `band` (`band[0] < ci_low` and `ci_high < band[1]`): with 95% confidence each outcome happens at least 5% of the time |
| `failing` | at least `min_runs` runs, every one failed: **a bug, not flakiness** |
| `mixed` | both outcomes, but too few to rule out a rare one (1 failure in 10 is 1.8%-40%) |
| `passing` | no run failed |
| `too_few` | fewer than `min_runs` runs |

`band=(0, 1)` calls every mixed cell flaky. The rows are sorted flakiest first:
by the lower bound on the rarer outcome, `min(ci_low, 1 - ci_high)` (it exceeds
`b` exactly when the interval lies inside `(b, 1 - b)`), then by failure rate (so
always-failing cells come before always-passing ones), then by labels.

`fail_rate` says how often runs that *passed through* a cell failed;
`fail_at_cell` says how many were *in* it when they failed. A run drives through
several cells, so a failure raises `fail_rate` in all of them; the cell with the
failures in it is the one where `fail_at_cell` is close to `runs_failed`. A cell
with a high `fail_rate` and `fail_at_cell` of 0 is one the failing runs only
passed through.

### Example

Twelve runs of `cut_in`, each 5 s slow then 5 s fast; in heavy rain the run always
fails once fast, in the dry one run in two does:

```python
report = openodd.merge_coverage(documents)
group = report.cell_flakiness[0]
group.to_dict()
# {"scenario": "cut_in", "environment": "sha256:3f9a...", "attributes": ["speed", "rain"],
#  "runs": 12, "runs_failed": 8, "failures_located": 8, "documents": [0, ..., 11],
#  "rows": [[["[0, 30)", "none"], 8, 4, 0.5, 0.215, 0.785, 0, 39.6, 0.0],
#           [["[30, 60]", "none"], 8, 4, 0.5, 0.215, 0.785, 4, 40.4, 0.0],
#           [["[0, 30)", "heavy"], 4, 4, 1.0, 0.510, 1.0, 0, 19.8, 0.0],
#           [["[30, 60]", "heavy"], 4, 4, 1.0, 0.510, 1.0, 4, 20.2, 0.0]]}
group.flaky_cells()               # the two dry cells: CellRow(...)
group.failing_cells()             # the two rainy ones
report.flaky_cells(min_runs=5, band=(0.1, 0.9))   # [(group, row), ...] over all groups
```

The report's Markdown (and `openodd-coverage`) gains a **Flakiness per ODD cell**
section, one table per group, at most 20 cells each:

```
### cut_in (sha256:3f9a...)

Runs: 12 (8 failed, 8 of them located in a cell). Cells visited: 4; flaky: 2; always failing: 2.

| speed x rain | Status | Runs | Failed | Fail rate | 95% interval | Failed here | Seconds | Meters |
|---|---|---|---|---|---|---|---|---|
| [0, 30) x none | flaky | 8 | 4 | 50% | 22%-78% | 0 | 39.6 | 0 |
| [30, 60] x none | flaky | 8 | 4 | 50% | 22%-78% | 4 | 40.4 | 0 |
| [0, 30) x heavy | failing | 4 | 4 | 100% | 51%-100% | 0 | 19.8 | 0 |
| [30, 60] x heavy | failing | 4 | 4 | 100% | 51%-100% | 4 | 20.2 | 0 |
```

Both dry cells fail half the time, but every failure happened while fast: the
slow cell's runs only passed through it. The JSON report (`to_dict()`,
`--json`) has the groups under `cell_flakiness`. With no outcomes, neither gains
anything.

**What it leaves out.** The per-case model's `flip_probability` is not attached to
the cells: it lives in the space of the parameters a run was chosen with, and a
cell is a region of the ODD's attributes that a run drives through -- a run with
fixed parameters visits many cells, and a cell has no point ("centre") in the
parameter space unless every parameter is one of its attributes and constant over
the run. Nor does the COD export (OpenODD 1.0 section 8.3) carry it: a COD is one
run's observed operational domain, sample by sample, while per-cell flakiness is a
statistic over many runs' outcomes, which section 8.3 has no column or manifest
entry for.

## Cost

Fitting is `O(n³)` in the runs observed, repeated for each step of the length-scale
refinement (three passes up to 400 runs, one beyond). In a release build it takes about
0.03 s at 40 runs, 0.5 s at 200 and 5 s at 400. A batch of 20 then takes well under a
second. The model is fitted again only after new runs are observed.
