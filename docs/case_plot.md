# Case plot: the drawn cases of a sweep

The [coverage view](coverage_view.md) shows the ODD's cells by the time runs
spent in them. The **case plot** shows the cases themselves. Every run of a
sampled sweep is placed at the values it was drawn with, along any two
attributes of its space, and marked by what it did. Like the coverage view,
it is a component for other tools' pages: HTML fragments with scoped CSS, or
the plot as JSON. It is in the same crate (`openodd_coverage_view::cases`
and `::case_plot`), and in Python `openodd.CasePlot`.

## What it reads

The coverage documents (`*_coverage.json`) of runs that recorded their case
(`CoverageCollector.record_case`, [flaky.md](flaky.md#recording-the-runs))
and their outcome (`record_outcome`):

- `case.values`: each attribute's drawn value, a number or a label.
- `case.buckets`: the ODD bucket of each value.
- `case.overrides`: the settings that made the case.
- `case.chosen_for`: why the case was drawn (`coverage`, `instability`, ...).
- `case.flaky`: the flakiness predicted for it, or null.
- `case.space`: each attribute's range or labels.
- `outcome.passed`, and `outcome.failed_at` when the run recorded it.

Runs with the same overrides in the same scenario ran the same case. They
are **one mark**, and its card lists every run in order. A document without
a case, or without an outcome, is not placed; a note under the plot counts
them.

## What it shows

| Mark | When |
|---|---|
| filled circle, green | every run of the case passed |
| cross, red | every run of the case failed |
| ring, yellow, with `n/m` | the runs did both: `n` of `m` passed |
| dashed outline around any of them | a flaky search chose the case (`chosen_for: instability`) |

A case run more than once is labelled `passes/runs`. The shape says what the
colour says, so colour is never the only signal. The colours are the coverage
view's (`--oo-pass`, `--oo-fail`, `--oo-flaky`).

- **Numeric attributes** use a linear scale over the space's range, widened
  to any value drawn outside it. Faint dashed grid lines mark the ODD's
  bucket edges, read from the attribute's cover item. The bucket labels sit
  along the axis when they fit.
- **Categorical attributes** are evenly spaced bands in the space's label
  order. Marks are jittered inside their band; the jitter is the same for the
  same case on every render.
- **Overlaps:** marks closer than a mark's width (13.0 m and 13.2 m, say)
  are pushed apart. A thin leader line and a dot lead from each moved mark to
  its value. The `n/m` labels go beside their mark where they hide no mark
  and no other label, or further out with a dashed leader line.
- **Hovering or focusing a mark** (tap on a phone) shows a card. It holds
  every value drawn and its bucket, each run's outcome in order (and when a
  failed run failed), why the case was chosen, and the predicted pass
  probability, flip probability, instability confidence and robustness
  radius when present.
- **Narrow screens:** the plot is drawn twice, wide and compact, and a
  container query shows the compact one below 560 px, so the labels stay
  readable on a phone. The legend wraps, and the table scrolls sideways in
  its own box.

Under the plot, **Repeated cases** lists the cases run more than once. Each
row has the case's values (the plotted attributes first), its outcomes in
order (`P`/`F`) and its verdict: flaky first, then failing, then passing.

## Embedding

```python
import openodd

plot = openodd.CasePlot(documents)          # the *_coverage.json contents
plot.attributes, plot.cases, plot.runs, plot.skipped

html = plot.html(x="dynamic.vehicle_ahead_relative_speed",
                 y="dynamic.vehicle_ahead_gap",
                 link="/cases?{axes}", id="cases")    # host-driven
table = plot.table_html(x=..., y=...)       # the repeated cases on their own
data = plot.data(x=..., y=...)              # the axes and every case, as a dict
page = plot.page("Drawn cases", x=..., y=...)          # standalone
```

The case plot shares the coverage view's styles and script. Put
`openodd.view.styles()` and `openodd.view.script()` on the page once for both.
Its root is `<div class="oo-cov oo-cases">`, so the coverage view's theme
handling applies: `prefers-color-scheme`, or `data-oo-theme="light|dark"` on
the root. The `--oo-*` variables apply too.

**Axis pickers** (*X →*, *Y ↑*) offer every attribute of the cases' space.
Picking the other axis's attribute swaps the two. A change dispatches the
same cancelable `openodd:axes` event as the coverage view, with `detail.x`,
`detail.y` and `detail.view == "cases"`. Unless the event is cancelled, the
page goes to `link`, where `{axes}` (or `{conditions}`) is replaced by
`x=...&y=...`. The host passes those back as `plot.html(x=..., y=...)`.

**Self-contained:** `explorer_html()` / `page()` render the asked pair and,
as hidden panels the pickers switch between, every other ordered pair of
attributes (at most `html::MAX_AXIS_PAIRS`, 100). Cards are written once per
component as JSON, not per panel. The command line does the same:

```bash
openodd-coverage results/ --cases-html cases.html \
    --cases-x dynamic.vehicle_ahead_relative_speed --cases-y dynamic.vehicle_ahead_gap
```

Without `--cases-x`/`--cases-y`, the first numeric attributes are plotted.
Tools that wrap `openodd-coverage` (`scenario-coverage` in
autoware_carla_scenario) take the same flags.

| Argument | Meaning |
|---|---|
| `x`, `y` | The horizontal and vertical attributes; by default, numeric ones first. |
| `scenario` | Only this scenario's cases. |
| `environment` | Only this environment's runs (`None`: those that recorded none). By default every environment, with a warning when more than one is pooled. |
| `table` | Whether the repeated cases' table follows the plot (default `True`). |
| `labels` | The component's words, by name. |

`labels={"title": "抽選ケース", "state_pass": "成功", "state_fail": "失敗",
"state_flaky": "不安定", "pass_short": "○", "fail_short": "×", ...}` replaces
words by name. The names are the fields of `case_plot::Labels` in the crate.
Text with placeholders keeps them: `runs_of` (`{n} runs`), `of_cases`,
`passes_of` (`{p} of {n} passed`), `failed_at` (`{t}`), `run` and `case`
(`{n}`), `more_runs`.

## In Rust

```rust
use openodd_coverage_view::{case_plot, cases::{CaseQuery, CaseSet}};

let set = CaseSet::from_documents(&documents);
let plot = set.plot(&CaseQuery { x: Some("speed".into()), y: Some("gap".into()), ..Default::default() });
let json = plot.to_json();                                           // draw it your own way
let fragment = case_plot::fragment(&set, &plot, &case_plot::Options::default());
let page = case_plot::page("Cases", &case_plot::explorer(&set, &CaseQuery::default(), &Default::default()));
```
