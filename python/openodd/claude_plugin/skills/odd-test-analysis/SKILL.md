---
name: odd-test-analysis
description: Analyse a simulation test campaign of an automated driving function against its ASAM OpenODD Operational Design Domain, from openodd_py coverage files (*_coverage.json) or their merged report. Use when asked to review, triage or explain test results, failures, flaky runs, ODD coverage gaps or scenario drift, or to write the analysis of an openodd test report.
---

# Analysing an ODD test campaign

A campaign is a set of runs, each a `*_coverage.json` file openodd's `CoverageCollector`
wrote: the scenario, the run's exposure to the ODD (per attribute bucket and per *cell*,
one bucket of each attribute), optionally the *case* the sampler drew and the *outcome*
(`passed`, `failed_at`, `failed_cell`, `environment`). The merged report adds coverage
grades, the flakiness of every ODD cell and the reassessed flakiness of every case.

## Where the figures come from

- **In the openodd analysis agent** (`openodd-test-report`, `openodd.agent`): the
  `mcp__openodd__*` tools read the report. Start with `summary`; then `failed_runs`,
  `run`, `odd_matrix`, `coverage_item`, `drift`, `markdown_report` where it points.
- **Elsewhere** (Claude Code in a repository with coverage files): build the same
  figures with the command line, then read them:
  `openodd-test-report <dirs> --no-agent --no-views --summary-json summary.json -o /dev/null`
  for the summary, `openodd-coverage <dirs>` for the Markdown report, and
  `openodd-coverage <dirs> --drift-html drift.html` or `openodd.CasePlot(docs).drift()`
  for the drift. Read single runs' JSON for their case and outcome.

Never state a number no tool or file gave you; quote it as given.

## Workflow

1. **Outcome.** Runs, passed, failed, without an outcome; per scenario and environment.
   No outcomes at all -> the verdict is `inconclusive`; describe coverage only.
2. **Where failures are.** Group the failed runs by `failed_cell` and by case values.
   Check `failing_cells` (every run that visited failed: a defect, not flakiness) and
   the `odd_matrix` of the two attributes the failures vary with most; drill into the
   worst cell with `conditions` for the next attributes.
3. **Systematic or flaky.** Use the rubric in [references/metrics.md](references/metrics.md):
   cell status from the Wilson interval, case metrics from the flakiness model, repeated
   cases (same case run more than once, same or different outcomes). Identical
   `failed_at` across many failures points to one scripted event.
4. **Coverage.** ODD grade per group and item; the holes (buckets no run reached);
   runs that left the ODD and time spent outside or "inside only because values were
   missing" (unmeasured attributes). A hole next to a failing region is a gap in the
   evidence, not a pass.
5. **Staging (drift).** When cases were recorded: per attribute, bias, MAE and bucket
   hit rate between drawn and measured. A run staged in another bucket than drawn was
   judged in a cell it was not aimed at; a `suggest` calibration names the knob fix.
6. **Verdict and write-up.** Follow [references/triage.md](references/triage.md):
   verdict rules, severities, how to phrase findings and recommendations.

## Pitfalls

- A cell's failure rate is about runs that *visited* it, not that failed *in* it:
  compare `fail_at_cell` with `runs_failed` before blaming the cell.
- `too_few` / tentative cells (fewer than 3 runs) prove nothing either way.
- A case chosen for instability (`chosen_for: instability`) was aimed at the pass/fail
  boundary: a high failure rate among them is expected and not by itself a regression.
- Flaky cases with `instability_confidence` just above 0.5 are boundary cases; rank by
  confidence and by whether a repeated run actually disagreed.
- Drift can explain apparent flakiness: two runs of the "same" case staged differently.
