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
and a backend's **Fuzzy rules** (`#rules`). They describe one chain:

| | Says | Example |
|---|---|---|
| ODD | what is talked about | `environment.rain` is `none`, `light`, `moderate` or `heavy` |
| Binding | how each attribute is measured | by the backend's probe `rain` |
| Backend manifest | how a run sets each bucket, and which rule inputs that writes | the knob `environment.rain` writes 70-100 for `heavy`; the rules read it as `precipitation` |
| Fuzzy rules | what follows from those inputs | `precipitation` is `dry`, `light`, `heavy` or `torrential` |

The manifest is what Python's `Backend.manifest()` writes ([rules.md](rules.md#the-backends-manifest)).
Load it with *Backend manifest*. Then the binding's probes are checked against
the backend's, and the attributes can be followed to the rules. *Examples →
CARLA weather* loads all four: a weather ODD, the CARLA backend's binding of
it, its manifest, and its weather rules.

- **Connections** (the binding editor's home) follows every attribute along
  the chain in one table. Its columns are the buckets, the probe, the knob and
  what it writes per bucket, the rule inputs, and the rule bases that read
  them. An attribute without a probe has a *Bind* button.
- **An attribute** shows its chain, ODD → binding → backend knob → fuzzy
  rules, and the form of its binding. The form has the probe (or a reader
  such as `measure`), unit, bucket edges or values, cover measure, least stay
  and text. Under the rules, a table compares each bucket with the term
  strongest at the middle of what its knob writes. It is marked when they
  differ: the rules' `heavy` may not be the ODD's `heavy`.
- **Binding & backend** sets the binding's name and the ODD files it
  measures, and lists the manifest's probes, knobs and rule inputs.

In the rules editor, *ODD names* names each input by the attribute that sets
it (`environment.rain` for `precipitation`), and each term chart gets a strip
of the ODD's buckets under it. The chart shows where each bucket's values
fall among the terms, with a line saying whether the terms are the buckets.
The file keeps the backend's names: those are what the backend's rules read.

The models are `binding.rs` (the binding and the manifest read and written,
and the chain followed, tested natively) and the interface `binding_app.rs`.

## Editing fuzzy rules

The **Fuzzy rules** switch in the toolbar (or `#rules` in the address) turns
the page into an editor of a backend's [fuzzy rules](rules.md): the rule file
a backend derives quantities from, such as the road's grip from how wet it
is. Every edit is read back by openodd's own reader, so the toolbar says
whether the Python package would accept the file, and where it would not.

- **The outline** lists the quantities and their rule bases. *Rule file*
  holds what the file `extends`, the files it extends (the browser cannot
  fetch them: load them there, found by their file name), and the files read
  with the SHA-256 of each.
- **A quantity** sets its name and `combine` (`min`, the default, or `max`),
  and shows each rule base's outputs at the tested values, the one `combine`
  takes in bold.
- **A rule base** has its inputs and its rules:
  - each input's universe and default, and its terms drawn over the universe.
    **Drag a corner** of a term to move it (corners keep their order); click
    the chart to test a value. Each term's shape (triangle, trapezoid,
    gaussian) and corners are in the table under it. *Spread evenly* lays the
    terms out so that each peaks where its neighbours end: rules naming one
    term each then read linearly between the peaks;
  - the rules as a table: a **term picker per input** (blank: any input
    value), a value per output, and how strongly the rule fires at the tested
    values. An antecedent with `OR`, `NOT` or nesting is edited as text (the
    `</>` button switches a row to text). Renaming an input or a term renames
    it in the rules; *A rule per term* adds one for each term of a lone input.
- **Test drive** (the panel): a slider per input of the quantity, each rule
  base's outputs and the combined ones, and the quantity's response as one
  input moves, the others held (combined solid, each rule base dashed).

A file that extends others may leave a rule base's inputs or rules to them:
the form says so, and *Write them here* copies what the extended files give
so they can be changed. A `null` that removes a quantity is not written by
the form; edit the YAML for it. Comments are not kept. **YAML** shows what the
form writes (each term and each rule's `then` on one line), and *Apply*
reads edited YAML back. The draft is kept in the browser apart from the ODD
editor's.

The model is `rules_doc.rs` (the rule file read and written, tested natively)
and the interface `rules_app.rs`.

## Embedding the studio

A page that has an element `id="openodd-studio"` gets the studio mounted in
it instead of in the body, so a page can wrap the studio with its own text.
The address picks the editor (`#binding`, `#rules`) when the page loads and
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
