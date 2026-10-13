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
thresholds as openodd's per-cell report (`stats`). "⚠ n" counts the runs
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

No I/O and no framework: JSON in, strings out. See
[docs/coverage_view.md](../../docs/coverage_view.md) for embedding.
