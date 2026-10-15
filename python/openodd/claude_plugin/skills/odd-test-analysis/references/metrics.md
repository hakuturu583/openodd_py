# The figures, and how to read them

## Outcome and coverage

| Figure | Meaning |
|---|---|
| `pass_rate` | passed / (passed + failed); runs without an outcome left out |
| item `grade` | covered buckets / buckets inside the ODD; a bucket is covered when its amount (hits, or the seconds, metres or entries `cover_by` names, counting only stays of `min_stay`) reaches `target` |
| group grade | mean of its items' grades (`odd` is the ODD's own attributes) |
| `holes` | buckets inside the ODD no run covered |
| ODD `seconds.inside` / `assumed` / `outside` | time inside the ODD; inside only because values were missing (the verdict rested on unmeasured attributes); outside |
| `runs_outside` | runs that left the ODD at least once: their failures there are outside the claim |

## ODD cells (`flaky_cells`, `failing_cells`, `odd_matrix`)

A cell is one bucket of each attribute. Per cell, over the runs with an outcome:
`runs_visited` (spent any step there), `runs_failed` (of those, failed), `fail_rate`,
its 95% Wilson interval `ci`, and `fail_at_cell` (failed while in it).

| Status | Rule (defaults: at least 3 runs, band 5%–95%) |
|---|---|
| `failing` | every visiting run failed -> a defect |
| `flaky` | both outcomes, and the whole 95% interval inside the band -> both happen at least 5% of the time |
| `mixed` | both outcomes, interval reaches outside the band -> not enough runs to call it |
| `passing` | none failed |
| `too_few` | fewer than 3 runs |

In the matrix, `state` is `fail`, `flaky`, `pass`, `outside` (outside the ODD),
`unknown` (only runs without an outcome) or `unsampled`; `tentative` marks too
few runs.

## Cases (`flaky_cases`, `run.case`)

The flakiness model is a Gaussian-process classifier fitted to every run of the scenario
with an outcome, over the parameters normalized to [0, 1] (numeric) or one-hot
(categorical, two categories 1 apart).

| Metric | Meaning | Reading |
|---|---|---|
| `pass_probability` | P(pass) at the case | < 0.2 likely fails, > 0.8 likely passes |
| `flip_probability` | chance a run here and one a small change away (radius 0.05) disagree | > 0.1 unstable; ~0.5 at a coin toss |
| `instability_confidence` | posterior probability that the flip rate exceeds 0.1 | > 0.5 listed as flaky; > 0.8 strongly |
| `robustness_radius` | smallest normalized change that flips the predicted outcome | < 0.05 on the boundary; `None` nothing flips it |

`predicted` is what the search expected when it chose the case; `assessed` is the
refit over all runs. A repeated case (same overrides) whose runs disagree is flaky by
observation, whatever the model says.

## Drift (`drift`)

Per attribute, drawn value against the value measured at the run's start (first sample
within 1 s): `bias` (mean measured - drawn), `mae`, `max_error`, `bucket_hit_rate`
(share of runs measured in the drawn bucket), and for a numeric one the fit
`measured = a * drawn + b` with `calibration.status`:

| Status | Meaning |
|---|---|
| `calibrated` | within noise of measured = drawn: no fix |
| `suggest` | a constant fix works: write the drawn value through the knob as `write_as` (`scale`, `offset`) |
| `too_few` | fewer than 5 runs |
| `flat` | the measured value does not follow the drawn one (`|a|` < 0.1): the knob does not work |
| `noisy` | residual too large for a constant fix |
| `structured` | the error depends on another attribute (e.g. rain): fix the scenario, not the knob |

A `bucket_hit_rate` below 1 means some runs were judged in cells they were not aimed at.
