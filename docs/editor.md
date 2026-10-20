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
