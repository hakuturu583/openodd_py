# openodd-coverage-view

An ODD coverage matrix for [openodd](../../README.md)'s coverage files, as a UI
component other tools embed.

It reads coverage documents (`openodd.coverage/1`): each run's time per ODD
cell, its outcome, and where it failed. It lays any two ODD attributes out as a
matrix. Clicking a cell keeps only the time spent there and lays the next two
attributes out inside it. Each cell is coloured by what its runs did:

| Colour | State | When |
|---|---|---|
| yellow | `flaky` | runs in it both passed and failed |
| red | `fail` | every run in it with an outcome failed |
| green | `pass` | every run in it with an outcome passed |
| grey | `outside` | a bucket of it is outside the ODD |
| white | `unsampled` | no run was in it |
| hatched | `unknown` | only runs without an outcome were in it |

A dashed outline marks a cell with too few runs to judge, by the same
thresholds as openodd's per-cell report (`stats`). The red "● n" badge counts the runs
that were in the cell when they failed.

```rust
use openodd_coverage_view::{data::Dataset, html, matrix::Query};

let data = Dataset::from_documents(&documents);       // serde_json::Value each
let m = data.matrix(&Query { conditions: vec![("rain".into(), "heavy".into())], ..Query::default() });
let json = m.to_json();                                // draw it your own way, or:
let opts = html::Options { link: Some("/coverage?{conditions}".into()), ..Default::default() };
let fragment = html::fragment(&data, &m, &opts);       // host-driven drill-down
let explorer = html::explorer(&data, &Query::default(), 2, &html::Options::default());
let page = html::page("Coverage", &explorer);          // standalone
```

The crate also draws the **drawn cases** of a sweep (each run's recorded `case`):
`cases::CaseSet` groups the runs of each case, and `case_plot` renders them as a
scatter over any two attributes, with a table of the cases run more than once
([docs/case_plot.md](../../docs/case_plot.md)):

```rust
use openodd_coverage_view::{case_plot, cases::{CaseQuery, CaseSet}};

let set = CaseSet::from_documents(&documents);
let plot = set.plot(&CaseQuery::default());
let fragment = case_plot::fragment(&set, &plot, &case_plot::Options::default());
```

No I/O and no framework: JSON in, strings out. See
[docs/coverage_view.md](../../docs/coverage_view.md) for embedding.
