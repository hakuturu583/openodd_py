# Comparing two sets of runs on the same ODD

Two sets of runs measured on the same ODD -- a **baseline** and another set,
say the software before and after a change -- laid over one another in both
views: the [coverage matrix](coverage_view.md) and the [case
plot](case_plot.md). In Rust it is `openodd_coverage_view::compare`; in the
browser, the WebAssembly `Comparison` ([wasm.md](wasm.md#comparing-two-sets-on-the-same-odd)).

## What changed

A cell of the matrix, or a case of the plot, is compared by its failure
rate: the share of its runs that failed, on each side.

| Change | When | Outline |
|---|---|---|
| **Regressed** ▼ | both sets ran there; the other set failed a larger share | red |
| **Improved** ▲ | both ran there; the other set failed a smaller share | green |
| **Unchanged** | both ran there; the same share failed | grey |
| **Newly covered** | only the other set ran there | dotted, accent |
| **No longer covered** | only the baseline ran there | dotted, grey |

A change is **beyond the runs' noise** when the two 95% Wilson intervals of
the failure rate do not overlap: a solid outline. Within the noise, the
outline is dashed. One pass against one failure is a dashed red outline, not
a solid one. Each change also has its arrow or sign, so colour never says it
alone.

## The matrix

Both sets are counted on the **same** matrix: the axes, buckets and
conditions are chosen from both sets together, and then each set's runs are
counted on them (`Dataset::matrix_on`). Every cell is split in two: the
baseline's runs on the left, the other set's on the right, each with its
state colour and icon, failure rate and `failed / runs`. The outline and a
badge (`▼ 40 pt`) say how the failure rate moved. The header counts each
set's runs, the regressed and improved cells, and the coverage gained and
lost. Clicking a cell and the axis pickers work as in the plain matrix
(`openodd:drill`, `openodd:axes`).

## The case plot

The runs of the same case (the same scenario and overrides) in either set
are **one mark**, at the case's values. The mark is a ring holding two
small marks -- the baseline's verdict on the left, the other set's on the
right (a dot where a set did not run it) -- and the ring says how the
case's failure rate moved, with a triangle down (worse) or up (better).
Its card lists both sets' runs in order, the change, and each set's
predicted pass probability.

Behind the marks, by default, is the **change of the predicted pass rate**:
openodd's flaky model fitted to each set's runs on its own, and the other
set's prediction minus the baseline's. Red where the other set is predicted
to pass less often, the plot's own surface where nothing moved, green where
more often. Where runs are few, both models are unsure, and the change
stays faint. Either set's own prediction (with its 30% and 70% contours) or
no background can be asked for instead.

Under the plot, **Changed cases** lists the cases that changed: regressed
first, then improved, no longer covered, newly covered; the larger change
first within each.

## The same ODD?

A comparison assumes both sets were measured on one ODD. It checks: each
coverage document records its ODD's name and structure digest
(`odd.name`, `odd.structure`), and each attribute's buckets. When the
digests differ, or an attribute's buckets differ or only one set has it,
both views say so above the grid, and the cells and cases are matched by
name only. `odd()` (or `Comparison::odd`) gives the details.

## In Rust

```rust
use openodd_coverage_view::{case_plot, compare::Comparison, html, CaseQuery, Query};

let cmp = Comparison::new(&before, &after);          // two slices of coverage documents
let m = cmp.matrix(&Query::default());               // m.deltas[row][column].change
let words = openodd_coverage_view::compare::Labels::default();
let matrix = html::compare::fragment(&cmp, &m, &html::Options::default(), &words);
let plot = cmp.plot(&CaseQuery::default());          // plot.cases[i].delta, .base, .other
let cases = case_plot::compare::fragment(
    &cmp, &plot, &Default::default(), &words,
    case_plot::compare::Background::None, [None, None],
);
```

A background needs each set's predictor, e.g. openodd's
`CasePredictor::new(&cmp.cases_of(Side::Base), &query)` and the other
side's.
