# The views in the browser: WebAssembly

`crates/openodd-wasm` builds the [coverage view](coverage_view.md), the
[case plot](case_plot.md) and the [fuzzy rules](rules.md) view for
`wasm32-unknown-unknown`. A web page computes
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

## Fuzzy rules

`RulesView` reads a backend's [fuzzy rules](rules.md) (the YAML text of a
rule file) and renders them as a component: for one quantity, a slider per
input, the outputs there -- the quantity's, and each rule base's, the one
`combine` takes marked -- the response chart (the outputs as one input moves
across its universe, each rule base dashed behind the quantity), and per rule
base each input's terms with the value chosen, and the rules with how
strongly each fires. The same engine as Python's `openodd.Rules` computes it.

```html
<div id="rules"></div>
<script type="module">
  import init, { RulesView, styles, script } from "./pkg/openodd_wasm.js";
  import { install, mount } from "./pkg/viewer.js";

  await init();
  install({ styles, script });
  const rules = new RulesView(yamlText, "weather_rules.yaml");   // throws on a bad file, saying where
  rules.merge(measuredText, "measured.yaml");                    // another file over it, quantity by quantity
  const view = mount(rules, document.getElementById("rules"), {
    view: "rules", quantity: "visibility", inputs: { fog_density: 80 }, x: "fog_density",
  });
  view.set({ inputs: { sun_altitude_angle: -10 } });
</script>
```

- **The sliders and pickers** dispatch `openodd:rules` (`detail.inputs`,
  `detail.quantity`, `detail.x`). `mount` cancels it, renders again, and puts
  the keyboard back on the control used. A slider dispatches on release and
  shows its value while it moves.
- **`options`** takes the query (`quantity`, `inputs`: `{name: value}`, `x`:
  the input the response chart moves) and the component's options (`id`,
  `labels`, `samples`: points per curve, 121 by default). An input not given
  reads its `default`, else the middle of its universe.
- **Words** (`labels`): `inputs`, `outputs`, `combined`, `response`, `vary`,
  `rules`, `firing`, `terms`, `quantity`, `combine`, `rule_base`,
  `rule_bases`.
- **Colours**: the coverage view's `--oo-*` variables and theme
  (`data-oo-theme`, or the reader's light or dark scheme); the terms' and rule
  bases' series through `--oo-rules-s0` to `s7`, the outputs' through
  `--oo-rules-o0` to `o3`.

| Method | Returns |
|---|---|
| `new RulesView(text, source)`, `merge(text, source)` | read a rule file (`merge`: another over it) |
| `quantities()` | a JSON array |
| `inputs(quantity)` | a JSON array of `{name, universe, default}` |
| `evaluate(quantity, inputs)` | the outputs at `inputs` (JSON objects) |
| `rules_html(query, options)` | the component |
| `rules_json(query, options)` | its data: each term's curve and degree, each rule's firing, the outputs, the response |

[`examples/wasm-viewer/rules.html`](../examples/wasm-viewer/rules.html) is an
editor: the rule file on one side, the component on the other, following
every edit (a file that does not read shows why and keeps the last view).

## Comparing two sets on the same ODD

`Comparison` reads two sets of coverage documents measured on the same ODD
-- a baseline and another set, say two versions of the software -- and lays
them over one another in both views ([compare.md](compare.md)). It has the
`Viewer`'s methods, so `mount` drives it the same way:

```js
import init, { Comparison, styles, script } from "./pkg/openodd_wasm.js";
const cmp = new Comparison(JSON.stringify(before), JSON.stringify(after), "v1.2", "v1.3");
mount(cmp, document.getElementById("cases"), { view: "cases", background: "delta" });
mount(cmp, document.getElementById("coverage"), { view: "coverage" });
JSON.parse(cmp.odd()).same;   // were both sets measured on the same ODD?
```

| Method | Returns |
|---|---|
| `new Comparison(base, other, base_name, other_name)` | read both sets (JSON arrays); `""` names them *Baseline* / *Compared* |
| `documents`, `split`, `cases` | counts: all documents, the baseline's, distinct cases |
| `attributes()`, `case_attributes()` | JSON arrays, of both sets |
| `odd()` | whether both sets were measured on the same ODD, and the attributes whose buckets differ |
| `coverage_html(query, options)`, `coverage_json(query)` | the matrix, each cell split in two, with its change |
| `cases_html(query, options)`, `cases_json(query, prediction)` | the case plot, each mark split in two; with `prediction`, both sets' grids and their change |

`options` takes the components' options, `names` (`[baseline, other]`),
the comparison's words in `labels` (`regressed`, `improved`, ...), and for
the case plot `background`: `"delta"` (the change of the predicted pass
rate, the default), `"base"`, `"other"` or `"none"`. A comparison has no
drift view; mount each set's `Viewer` for it.

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
`make_sample.py`). It shows both views with live axis pickers. Below them,
it compares two sets: the sample and `sample_compare.json` (the same cases
run by a "next version" that is worse in heavy rain and better in none), or
two sets of files picked. A module page
cannot be opened from `file://`, so serve the folder:

```bash
examples/wasm-viewer/build.sh
python3 -m http.server -d examples/wasm-viewer 8000   # http://localhost:8000
```

The URL hash can choose the starting point:

- `#data=URL` loads other coverage JSON.
- `#compare=URL` compares it (or the sample) with other coverage JSON.
- `#x=...&y=...` and `#cx=...&cy=...` set the starting axes of the case plot
  and of the matrix.
- `#pick-x=...&pick-y=...` then changes the case plot's axes through its own
  pickers, for a scripted check.
