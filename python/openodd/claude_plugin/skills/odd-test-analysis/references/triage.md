# Verdict, severities and the write-up

## Verdict

- `fail`: any run failed for a reason that is not plainly flakiness: a `failing` cell,
  a region where the failures concentrate, a repeated case failing every time, or a
  failure rate above the pass criteria the user gave.
- `inconclusive`: no run recorded an outcome, or the evidence is too thin to call
  (only `too_few` / `mixed` cells, or a handful of runs).
- `pass`: no failures, or only failures explained as flakiness at the boundary, with
  the ODD covered well enough for the claim (say what is not covered).

Pass criteria the user states (a required pass rate, a coverage target) override these.

## Severity

| Severity | Use for |
|---|---|
| `critical` | systematic failures inside the ODD: failing cells, a region failing every time, a repeated case failing every time |
| `major` | flaky cells with many runs; coverage holes in or next to a failing region; staging drift that moves runs to other cells; runs leaving the ODD or verdicts resting on unmeasured attributes |
| `minor` | boundary flakiness with few runs; small coverage holes away from failures; drift that keeps runs in their buckets |
| `info` | context: what was run, what was not recorded |

## Findings

- One finding per cause, most severe first. Title: the claim in a few words.
- `detail`: what, where in the ODD (attribute = bucket), how many runs, and why it is
  systematic or flaky. Name documents by index (`doc 12`) or file.
- `evidence`: the figures, quoted as the tools gave them
  (`[0,10) x [-15,-5): runs 11, failed 11, ci 0.741-1.0`). No number that is not there.
- Separate what the data shows from what you infer ("suggests", "consistent with").

## Recommendations

Concrete next steps, each tied to a finding:

- the cells to sample next (holes, `mixed` / `too_few` cells next to failures);
- the cases to re-run (flakiest by confidence, repeated cases that disagreed) and how
  many times;
- the knob to recalibrate, with the suggested scale and offset;
- the defect to investigate, with a representative failing run.

## Style

Write for a reviewer who has not seen the data: the headline is one sentence with the
outcome and the main cause; the summary is one paragraph; keep findings short. Write in
the language asked for; keep attribute names, bucket labels and figures as given.
