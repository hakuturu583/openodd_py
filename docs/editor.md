# Editing an ODD in the browser

`crates/openodd-editor` is an editor for ASAM OpenODD 1.0 YAML, written in Rust
([Leptos](https://leptos.dev)) and compiled to WebAssembly. It runs entirely in
the browser: nothing is uploaded. Demo: <https://hakuturu583.github.io/openodd_py/>.

The document is edited as forms, and checked as it is edited by openodd's own
reader -- the core crate, built without its `native` feature -- so what the
editor accepts is what the Python package reads.

## Seeing the ODD while editing it

Every view is drawn from openodd's own verdict. A value is **allowed** (green)
when some values of the other attributes put it inside the ODD, and
**excluded** (red) when nothing can. Numbers are sampled along their line, so
the colour changes exactly where a condition cuts (`<= 2`, `[0 .. 40] km/h`).

- **ODD map** (the home view):
  - counts of attributes, restricted attributes and modules, and the share of
    the domain excluded;
  - the **structure graph**: roots → modules and labels → attributes, include
    arrows blue, exclude orange, labels dashed. Hover to trace a node, click to
    edit it;
  - the **ODD over two attributes**: any pair as a grid of allowed / excluded
    cells. Click a cell to test it;
  - an **attribute card** per attribute, grouped by the taxonomy's top level,
    with its domain coloured and the modules that test it.
- **A concept** shows where the ODD stands on it above its form; a
  categorical's literals carry the same colour.
- **A module** shows what it accepts of each attribute it tests, the others
  aside, and which modules read it and which it reads. Each numeric condition
  has a number line under it: **drag its ends** to change the value.
  Booleans, modules and labels are switches; a categorical's literals are
  chips.
- **Test drive** (the panel): set the values a run would measure. Drag a
  marker on a line, click a literal, or click a cell of the map. The verdict
  shows inside, outside, or inside only if the unknowns allow. It also shows
  each module's verdict, which colours the graph and the outline, and the
  derived categoricals' values. *Fill a point inside* picks an allowed value
  of each attribute.

Icons follow what a concept's name says it is (a road, the rain, a pedestrian,
a speed), else its kind.

## What it edits

- **TAXONOMY** as a tree:
  - groups (records and containers);
  - numbers (`<primitive> <unit type>`);
  - booleans;
  - categoricals (ordered literals);
  - derived categoricals (literals defined by expressions on other concepts);
  - references to another type's id;
  - anything else, as YAML.
- **MODULES** and **ODD** (the roots): `TITLE`, `DESCRIPTION`, `ACTIVE`,
  `LABEL(S)`, `METADATA`, one include and one exclude section (all / any),
  each with its one nested section of the other operator.
- **IMPORT**, and `conversion` / `COD` / `OD` kept as YAML.

Renaming a concept or a module renames the conditions that name it. Comments
are not kept: YAML readers drop them.

The **YAML** tab shows what the document writes. Edit it there and *Apply* to
read it back. **Details** shows openodd's reading: the revision
([versioning.md](versioning.md)), the version, the roots, the modules'
normalised conditions and every bucket. To bucket every attribute, the check
measures each concept in the unit its conditions are first written in, as a
binding file would.

The files a document `IMPORT`s can be loaded under *Document & imports*. The
check reads them as a transmission, by name, as the Python package does. The
draft is kept in the browser's local storage. *Download YAML* saves the file.

## The binding and the connections

The studio holds three documents at once, each with its own editor, chosen in
the toolbar: the **ODD** (`#` in the address), its **Binding** (`#binding`),
and a backend's **Rules** (`#rules`). They describe one chain:

| | Says | Example |
|---|---|---|
| ODD | what is talked about | `environment.rain` is `none`, `light`, `moderate` or `heavy` |
| Binding | how each attribute is measured | by the backend's probe `rain` |
| Backend manifest | how a run sets each bucket, and which rule inputs that writes | the knob `environment.rain` writes 70-100 for `heavy`; the rules read it as `precipitation` |
| Rules | what follows from those inputs | at `precipitation` 70, visibility is 100 to 220 m |

The manifest is what Python's `Backend.manifest()` writes ([rules.md](rules.md#the-backends-manifest)).
Load it with *Backend manifest*. Then the binding's probes are checked against
the backend's, and the attributes can be followed to the rules. *Examples →
CARLA weather* loads all four: a weather ODD, the CARLA backend's binding of
it, its manifest, and its weather rules.

- **Connections** (the binding editor's home) follows every attribute along
  the chain in one table. Its columns are the buckets, the probe, the knob and
  what it writes per bucket, the rule inputs, and the parts that read them.
  An attribute without a probe has a *Bind* button.
- **An attribute** shows its chain, ODD → binding → backend knob → rules, and
  the form of its binding. The form has the probe (or a reader such as
  `measure`), unit, bucket edges or values, cover measure, least stay and
  text. Under the rules, a table says what each bucket gives through each
  part reading its input: the least and greatest value of the quantity over
  the values the knob writes for that bucket (heavy rain, 70 to 100: a
  visibility of 60 to 220 m).
- **Binding & backend** sets the binding's name and the ODD files it
  measures, and lists the manifest's probes, knobs and rule inputs.

In the rules editor, *ODD names* names each input by the attribute that sets
it (`environment.rain` for `precipitation`), and each part's chart gets a
strip of the ODD's buckets under its input, with what each bucket gives
written beside it. The file keeps the backend's names: those are what the
backend's rules read.

The models are `binding.rs` (the binding and the manifest read and written,
and the chain followed, tested natively) and the interface `binding_app.rs`.

## Editing rules

The **Rules** switch in the toolbar (or `#rules` in the address) turns the
page into an editor of a backend's [rules](rules.md): conditional
distributions, the rule file a backend derives quantities from, such as the
road's grip from how wet it is. Every edit is read back by openodd's own
reader, so the toolbar says whether the Python package would accept the file,
and where it would not.

- **The outline** lists the quantities and their parts. *Rule file* holds the
  inputs the file describes (a default, a line saying what each is), what the
  file `extends`, the files it extends (the browser cannot fetch them: load
  them there, found by their file name), and the files read with the SHA-256
  of each.
- **A quantity** sets its name, a line saying what it is, and `combine`
  (`min`, the default, or `max`). It shows each part's low, high, median and
  value at the tested quantile, and the quantity's; the part that gives the
  quantity's value at the quantile is marked.
- **A part** has its inputs and points, and its distribution:
  - the inputs it reads and the **points** of each, increasing. Changing the
    points reads the values at the new points from the old ones, so adding a
    point keeps the part as it was;
  - the distribution's family (uniform, triangular, normal). Switching makes
    each point's distribution from the one there (a uniform's ends become a
    triangle's, its mode the middle);
  - a **chart** over the first input, the others held at the tested values:
    the range low to high, the median, and the value at the tested quantile.
    For a part over one input, each value written at a point is a handle:
    **drag it up or down** (`low` stays below `mode` and `high`). Click the
    chart to test an input value;
  - the **values** as a table: a row per point over one input, a grid per
    parameter over two. A part over more inputs is edited in the YAML.
- **Test drive** (the panel): a slider per input of the quantity and one for
  the quantile `p`, each part's and the quantity's values, and the quantity's
  response as one input moves, the others held (the range shaded, the value
  at `p` solid, each part's value at `p` dashed).

A file that extends others may leave a part's points or distribution to them:
the form says so, and *Write them here* copies what the extended files give so
they can be changed. A `null` that removes a quantity is not written by the
form; edit the YAML for it. Comments are not kept. **YAML** shows what the
form writes (each parameter's values on one line), and *Apply* reads edited
YAML back. The draft is kept in the browser apart from the ODD editor's.

The model is `rules_doc.rs` (the rule file read and written, tested natively)
and the interface `rules_app.rs`.

## The factor graph

The **Graph** switch (or `#graph`) shows the same rules as a factor graph:
the **variables** -- the inputs the backend gives (outlined) and the
quantities the rules derive (filled) -- and the **factors** between them
(squares), each a part: the distribution of its quantity given the variables
it reads. A part may read a quantity ([chains](rules.md#chains)): that link
is dashed, and the graph runs left to right, each quantity after those it
reads. Each variable shows its value at the tested inputs (a quantity at its
median), each factor its distribution's family.

- **Drag a node** to move it; *Arrange* lays the graph out in layers again.
  Where each node is is kept in the browser, by name.
- **Drag from a variable's port** (the dot on its right) onto a factor to make
  that part read the variable too (a new last input, the distribution the
  same along it), or onto a quantity to give it a new part reading the
  variable. A link that would make a quantity read itself, through any chain,
  is refused, with the chain it would close.
- **Select** a factor, a quantity or a link to edit it beside the canvas: a
  factor and a quantity open the rules editor's own form (points, the
  distribution and its chart, the values); an input its name (renaming it
  renames every part reading it), default and description, and the factors
  reading it; a link removes the input from the part (its distribution is the
  one at the input's first point), or the part when it is its only input.
  With nothing selected, the panel is the rules editor's test drive.

Renaming a quantity, in either view, renames it in every part that reads it.
The interface is `graph_app.rs`; the layout (`auto_layout`) puts the inputs in
the first column, and for each depth of the chain a column of factors and one
of quantities.

### Fitting the graph to data

With nothing selected, the graph's side panel has a **Fit to data** card: it
fits every factor's distribution to rows measured in the real world, by
maximum likelihood, in the browser -- the same code as Python's `Rules.fit`
([rules.md](rules.md#fitting-to-data)).

1. **Load data (JSON)**: each row the values seen together. A row may leave
   any value out (or give it `null`); keys the rules do not read are ignored.

   ```json
   {
     "rows": [
       {"water": 70, "fog": 10, "tire_friction": 0.62, "visibility": 310},
       {"water": 20, "tire_friction": 0.85},
       {"fog": 80, "visibility": null, "dt": 0.5}
     ],
     "weight": "dt"
   }
   ```

   A bare list of rows reads too, and so do columns (`{"water": [...], ...}`).
   `weight` (optional) names the column weighing each row, 1 where a row has
   none.
2. The card shows, per variable, how many rows give it, and per factor how
   many rows it **reads** (they give its quantity and everything it reads)
   and how many it **skips** (they give its quantity but miss something it
   reads). A factor no row reads is greyed and left as it is.
3. **Fit** runs the fit in a Web Worker, so the page stays free while it
   runs: a bar shows how far it has come -- the quantity being fitted, the
   share done and about how long is left, from the pace so far -- and
   **Cancel** stops it. The share done is an estimate: each quantity weighs
   its rows, parts and the stages of its fit, and an iteration count is not
   known ahead. When it ends, the fitted parameters are written into the
   document, every point of every part fitted; the YAML, the charts and the
   graph follow. The table
   below gives each quantity's rows, its log-likelihood before and after,
   the values estimated and whether the fit converged. **Undo fit** puts the
   rules back as they were.

A part's form has a **Fit holds** card: what a fit keeps as written -- all of
the part, a parameter at every point (a normal's `low` and `high` always
are), or values at points (`mean@water=0`, `@water=100`). It is the part's
`fixed:` ([rules.md](rules.md#holding-values)); a factor holding something
says so on the canvas (`· fixed`, `· part fixed`). The data is kept in the
browser with the draft.

The worker is its own small program, `src/bin/fit_worker.rs`, which Trunk
builds beside the editor (`index.html`: `data-type="worker"` with a loader
shim) and the page starts as `fit_worker_loader.js`. It receives the rules
as YAML, the files they extend and the data, and posts its progress (at
most every 50 ms) and the fitted parts back as JSON (`fit_job.rs`, tested
natively). A page that embeds the studio without the worker's files still
fits: when the worker does not start, the fit runs on the page's thread,
the bar runs without a share, and the page waits until it is done.

## Embedding the studio

A page that has an element `id="openodd-studio"` gets the studio mounted in
it instead of in the body, so a page can wrap the studio with its own text.
The address picks the editor (`#binding`, `#rules`, `#graph`) when the page loads and
whenever it changes, so the page can switch it. The studio's stylesheet sets
`:root` and `body`; scope it to the element when the page has its own.

## Building

```bash
rustup target add wasm32-unknown-unknown
cargo install trunk                 # or a release binary
cd crates/openodd-editor
trunk serve                         # http://127.0.0.1:8080, rebuilt on change
trunk build --release --cargo-profile wasm-release   # into dist/
```

Trunk builds two programs: the editor, and the fit's worker
(`fit_worker`, with its `fit_worker_loader.js`); both land in `dist/`.

The `wasm-release` profile (in the workspace `Cargo.toml`) optimises for size:
the page is under 2 MB of WebAssembly, about 770 KB gzipped.

The model is plain Rust, tested natively with `cargo test -p openodd-editor`:
`doc.rs` reads YAML into the editor's document and writes it back, and
`check.rs` holds the check, the per-bucket, per-module and sampled verdicts,
and the evaluator. The drawing is in `viz.rs` and the icons in `icons.rs`.

## The demo site

`.github/workflows/pages.yml` builds the editor and the
[coverage viewer](wasm.md) (under `viewer/`) on every push to `main`, and
deploys them to GitHub Pages. Pages must be set once to deploy from GitHub
Actions (*Settings → Pages → Build and deployment → Source: GitHub Actions*).
