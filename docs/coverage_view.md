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
next two attributes inside it. The breadcrumb leads back up. In the explorer
each level is one crumb (*Whole ODD › rain = heavy, speed = [30, 60) › ...*).
A host-driven fragment only knows its conditions, not the levels they came in,
so there each condition is a crumb of its own (*Whole ODD › rain = heavy ›
speed = [30, 60)*); the row's condition comes before the column's.

**Choosing the axes.** Each axis has its picker on its own edge of the grid:
*Columns →* above the column labels, *Rows ↓* in the corner above the row
labels (*None* for a single row). Picking the other axis's attribute swaps
the two. Hovering a cell
highlights its row and column labels. They
offer the attributes not already clicked into. When left alone, a level lays
out the first two of those in the ODD's order.

The header's run count is every run inside the conditions, whichever axes are
laid out. With `min_seconds`, a run counts there when its stay in the whole
region reaches it, so a run that stayed briefly in several cells can count in
the header and in no single cell.

### Runs without outcomes (logs)

Runs measured on recorded logs -- a drive log replayed through the ODD's
attributes -- have cells but no outcome: there is nothing to judge, only where
they went. When no run the matrix selects recorded an outcome
(`matrix["outcomes"]` is false), it says so and shows exposure instead:

- each visited cell is shaded on one blue ramp by the time the runs stayed in
  it (by how many runs were in it, when no time was recorded), relative to the
  cell with the most, on a square-root scale; it reads that time and its runs;
- the summary is the runs, how much of the ODD they covered, the time (and
  distance) recorded, and the *thinly covered* cells: visited by fewer than
  `min_runs` runs, which a dashed outline marks;
- nothing about failures: no failure rate, flaky or failing cells, or badge.

One run with an outcome is enough for the usual view; the runs without one
are then counted as *covered, no outcome*. The ramp's hue is `--oo-heat`.

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

The axis pickers work the same way. A change dispatches a cancelable
`openodd:axes` event with `detail.conditions`, `detail.x` and `detail.y`
(`y` is `""` for a single row). Unless it is cancelled, the page goes to the
link with `x=...&y=...` added to the level's conditions. The host passes them
back as `view.html(x=..., y=...)`; an empty `y` means one row, and a missing
one means the default. Without a link and without a handler, the pickers do
nothing.

```js
document.addEventListener("openodd:axes", (event) => {
  event.preventDefault();
  load(event.detail.conditions, event.detail.x, event.detail.y);
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
matrix inside every visited cell, as hidden panels that a click shows. Its first
level is rendered for every pair of rows and columns the pickers offer (up to
100 pairs, `html::MAX_AXIS_PAIRS`), so they switch without a server; the
levels below lay out their default axes. It
renders at most 400 panels (`html::MAX_PANELS`); past that, a cell has nothing
to open, and its panel says so. With many attributes and buckets, ask for
fewer levels, or start from a cell (`conditions`). Nothing
needs serving. The command line does the same:

```bash
openodd-coverage results/ --html coverage.html --html-depth 2
```

Give each component on a page its own `id`: its panels' ids derive from it.
A condition's attribute name must not contain `=`, which separates it from
the bucket in a link.

### Selecting runs and thresholds

Every call takes the same query:

| Argument | Meaning |
|---|---|
| `x`, `y` | The column and row attributes. When left out, the first free ones in the ODD's order; `y=""` gives a single row. |
| `conditions` | The cells clicked into, outermost first: `[(attribute, bucket), ...]` or a mapping. |
| `scenario` | Only this scenario's runs. |
| `environment` | Only this environment's runs (`None` for those that recorded none). When left out (`openodd.view.ANY`), every environment, and the component warns if more than one is pooled. |
| `min_runs`, `band` | When a cell is judged rather than tentative. |
| `min_seconds` | The least a run must stay in a cell to count as in it. |

The component's words are English. `labels={"title": "ODD カバレッジ",
"state_flaky": "...", ...}` replaces them by name; the names are the fields
of `html::Labels` in the crate.

The drawn cases of a sweep -- each run at the values it was drawn with --
are the [case plot](case_plot.md), on the same styles and script.

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
