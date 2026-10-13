# Coverage view: the ODD matrix as an embeddable component

The `openodd-coverage-view` crate (`crates/openodd-coverage-view`, in Python
`openodd.CoverageView`) shows how a set of runs covered the ODD, and how they
fared in each part of it. It is a building block for other tools' pages, not
an application: it renders HTML fragments with scoped CSS, or a matrix as
JSON, and leaves the page, the routing and the data loading to the host.

## What it shows

A matrix of two ODD attributes: one along the columns, one along the rows.
Each cell is a pair of their buckets, and holds the runs that spent time there:

| Colour | When |
|---|---|
| yellow | **flaky**: runs in it both passed and failed |
| red | **fails**: every run in it failed |
| green | **passes**: every run in it passed |
| grey | **outside the ODD**: a bucket of the cell is outside it |
| white | **not sampled**: no run was in it |
| hatched | **covered, no outcome**: only runs that recorded no outcome were in it |

A cell shows its failure rate and `failed/runs`. The red "● n" badge counts the runs that
were in the cell *when they failed* (`outcome.failed_cell`, recorded by
`record_outcome(failed_at=...)`). A run's verdict is the whole run's, so a run
that failed elsewhere still counts as failed in every cell it drove through.
That is why a slow cell can be red with no badge: the runs that drove through it
all failed, but later, somewhere else.

Each state also has an icon (✓ passes, ✕ fails, ∿ flaky, ⊘ outside), so colour
never says it alone. Hovering (or focusing) a cell shows a card with its runs,
failures, failures in it, failure rate, interval and time. The header sums up
the level: its runs, how much of the ODD they covered, and how many cells are
flaky or failing, with a bar showing the share of cells in each state.

A **dashed outline** marks a cell with too little evidence to judge. That is
fewer than `min_runs` runs, or both outcomes but a 95% Wilson interval reaching
outside `band`. The thresholds are the ones openodd's per-cell report uses
(`CELL_MIN_RUNS`, `CELL_BAND`, [flaky.md](flaky.md#flakiness-per-odd-cell)).
The colour still says what the runs did.

**Clicking a cell** keeps only the time spent in that cell and lays out the
next two attributes inside it. In the breadcrumb, *Whole ODD › rain = heavy,
speed = [30, 60) › ...*, each level leads back up.

## Embedding

```python
import openodd

view = openodd.CoverageView(documents)        # the *_coverage.json contents
view.attributes, view.scenarios, view.environments
```

Put `openodd.view.styles()` (in a `<style>`) and `openodd.view.script()` (in a
`<script>`) on the page once. Every class is prefixed `oo-` and scoped under
`.oo-cov`. The colours are CSS variables (`--oo-flaky`, `--oo-fail`,
`--oo-pass`, `--oo-outside`, `--oo-unsampled`, `--oo-unknown`, `--oo-ink`,
`--oo-line`, ...) for the host to override. The component follows
`prefers-color-scheme`, or `data-oo-theme="light|dark"` on its root.

### Host-driven: one level per request

```python
html = view.html(
    conditions=openodd.view.parse_conditions(request.args.getlist("at")),
    link="/coverage?{conditions}",          # each cell links to the level below
    id="odd-matrix",
)
```

Each cell the host can click into:

- is a link: `{conditions}` becomes `at=attribute%3Dbucket&at=...`, which
  `parse_conditions` reads back, so a server-rendered page needs no script.
  With htmx, add `hx-boost` around it, or target the fragment's root (`id`).
- carries its conditions as `data-oo-drill` (JSON `[[attribute, bucket], ...]`).
- on click, dispatches a bubbling, cancelable `openodd:drill` event. Its
  `detail.conditions` holds the conditions and `detail.root` the root's id.
  A single-page host cancels it (`event.preventDefault()`), fetches the next
  level its own way and swaps the fragment.

```js
document.addEventListener("openodd:drill", (event) => {
  event.preventDefault();
  load(event.detail.conditions);   // e.g. render view.html(conditions=...) again
});
```

`view.matrix(...)` returns the same matrix as a dict, for a host that draws
it itself. It holds the axes and their labels, each cell's `state`,
`tentative`, `runs`, `failed`, `fail_at_cell`, `fail_rate`, `ci_low` and
`ci_high`, plus the totals and the counts per state.

### Self-contained: a static report

```python
open("coverage.html", "w").write(view.page("ODD coverage", depth=2))
```

`explorer_html(depth=...)` renders the matrix and, `depth` levels down, the
matrix inside every visited cell, as hidden panels that a click shows. Nothing
needs serving. The command line does the same:

```bash
openodd-coverage results/ --html coverage.html --html-depth 2
```

### Selecting runs and thresholds

Every call takes the same query:

| Argument | Meaning |
|---|---|
| `x`, `y` | The column and row attributes. When left out, the first free ones in the ODD's order; `y=""` gives a single row. |
| `conditions` | The cells clicked into, outermost first: `[(attribute, bucket), ...]` or a mapping. |
| `scenario` | Only this scenario's runs. |
| `environment` | Only this environment's runs (`None` for those that recorded none). When left out, every environment, and the component warns if more than one is pooled. |
| `min_runs`, `band` | When a cell is judged rather than tentative. |
| `min_seconds` | The least a run must stay in a cell to count as in it. |

The component's words are English. `labels={"title": "ODD カバレッジ",
"state_flaky": "...", ...}` replaces them by name; the names are the fields
of `html::Labels` in the crate.

## In Rust

```rust
use openodd_coverage_view::{data::Dataset, html, matrix::Query};

let data = Dataset::from_documents(&documents);
let m = data.matrix(&Query::default());
let fragment = html::fragment(&data, &m, &html::Options::default());
```

The crate depends only on `serde_json` and `indexmap`. It has no I/O and no
framework. openodd's per-cell report (`cell_flakiness`) takes its Wilson
interval and statuses from this crate's `stats` module, so the matrix and the
report never disagree.
