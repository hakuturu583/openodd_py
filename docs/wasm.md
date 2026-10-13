# The views in the browser: WebAssembly

`crates/openodd-wasm` builds the [coverage view](coverage_view.md) and the
[case plot](case_plot.md) for `wasm32-unknown-unknown`. A web page computes
them from coverage documents in the browser. There is no Python and no
server, and nothing is uploaded. The case plot's background is openodd's own
flaky model, fitted in the browser: the same Gaussian-process classifier,
defaults and seed (`"report"`) as the merged report. A sweep gives the same
background in Python and in the browser.

## Building

```bash
rustup target add wasm32-unknown-unknown
cargo install wasm-pack            # or your package manager's
examples/wasm-viewer/build.sh      # wasm-pack build --target web, into examples/wasm-viewer/pkg
```

The package (`pkg/`) is `openodd_wasm.js` and `openodd_wasm_bg.wasm`: about
760 KB, 280 KB gzipped. Next to them goes `viewer.js` (from
`crates/openodd-wasm/js/`), the small module that keeps a view live.

The core builds without its default `native` feature. That feature is the
part that needs an operating system: the file lock and `tar` of git sources,
and tar.gz ODD archives. Without it, those calls return an error. The Python
package always has it.

## Embedding

```html
<div id="cases"></div>
<div id="coverage"></div>
<script type="module">
  import init, { Viewer, styles, script } from "./pkg/openodd_wasm.js";
  import { install, mount } from "./pkg/viewer.js";

  await init();
  install({ styles, script });                       // the CSS and script, once per page
  const viewer = new Viewer(JSON.stringify(documents));  // the *_coverage.json contents

  const plot = mount(viewer, document.getElementById("cases"), {
    view: "cases", x: "dynamic.vehicle_ahead_relative_speed", y: "dynamic.vehicle_ahead_gap",
  });
  mount(viewer, document.getElementById("coverage"), { view: "coverage" });
  mount(viewer, document.getElementById("drift"), { view: "drift" });   // drawn vs measured
  plot.set({ y: "environment.fog" });                // or let the reader use the pickers
</script>
```

`mount(viewer, element, options)` renders a view into the element and keeps
it live:

- **The case plot's axis pickers** dispatch `openodd:axes`. `mount` cancels
  the event and renders the chosen pair at once. A new pair costs only its
  grid: the model is fitted once per selection of runs, and each pair's grid
  is cached.
- **The coverage matrix's pickers and cells** work the same way through
  `openodd:axes` and `openodd:drill`. Clicking a cell renders the level
  inside it.
- **After each render**, the element receives `openodd:rendered`, with
  `detail.view`, `detail.state` (the query) and `detail.ms`.
- **`options`** takes the query (`x`, `y`, `conditions`, `scenario`,
  `environment`, ...) and the component's options:
  - `id`
  - `labels`: the same names as in Python, e.g. `{"predicted_scale": "予測成功率"}`
  - `table`
  - `prediction`: `false` for no background
  - `instability_rings`
  - `basis` (`"drawn"` / `"measured"`, in the query), `measured_at`, `drift_leaders`

The components' words are English by default; `labels` replaces them.

Without `mount`, call the `Viewer` directly. Each method takes the query
and the options as JSON strings:

| Method | Returns |
|---|---|
| `new Viewer(json)`, `add(json)` | read documents (a JSON array; `add` appends) |
| `attributes()`, `case_attributes()`, `scenarios()` | JSON arrays |
| `documents`, `cases` | counts |
| `cases_html(query, options)` | the case plot, with its background |
| `cases_json(query, prediction)` | the plot as data; with `prediction`, the grid with every cell's full metrics, and each case's `predicted_pass_probability` |
| `coverage_html(query, options)`, `coverage_json(query)` | the coverage matrix |
| `drift_html(query, options)`, `drift_json(query, options)` | the scenario's realisation: drawn against measured, with the knob calibration ([drift.md](drift.md)); `options.knobs` = `{attribute: [scale, offset]}` |
| `styles()`, `script()` | the components' CSS and script |

The background needs only each cell's pass probability: one posterior mean
and variance per cell, not the 32 neighbours and 256 posterior draws behind
the flip probability. That keeps it interactive. `cases_json(query, true)`
and Python's `CasePlot.prediction()` measure every cell fully.

## Timing

Measured in V8 (Node 22) with the release build, on 49 runs of a cut-in
sweep (31 cases, 5 parameters):

| Step | Time |
|---|---|
| instantiate the module | 17 ms |
| read the documents | 31 ms |
| first case plot: fit the model, its 48 × 48 grid, render | 123 ms |
| another pair (relative speed × fog) | 3 ms |
| the same pair again (cached) | 3 ms |
| the coverage matrix | 3 ms |

## The demo

`examples/wasm-viewer/index.html` loads coverage files from a file picker or
by drag-and-drop, or the bundled synthetic sample (`sample.json`, written by
`make_sample.py`). It shows both views with live axis pickers. A module page
cannot be opened from `file://`, so serve the folder:

```bash
examples/wasm-viewer/build.sh
python3 -m http.server -d examples/wasm-viewer 8000   # http://localhost:8000
```

The URL hash can choose the starting point:

- `#data=URL` loads other coverage JSON.
- `#x=...&y=...` and `#cx=...&cy=...` set the starting axes of the case plot
  and of the matrix.
- `#pick-x=...&pick-y=...` then changes the case plot's axes through its own
  pickers, for a scripted check.
