# Editing an ODD in the browser

`crates/openodd-editor` is an editor for ASAM OpenODD 1.0 YAML, written in Rust
([Leptos](https://leptos.dev)) and compiled to WebAssembly. It runs entirely in
the browser: nothing is uploaded. Demo: <https://hakuturu583.github.io/openodd_py/>.

The document is edited as forms, and checked as it is edited by openodd's own
reader -- the core crate, built without its `native` feature -- so what the
editor accepts is what the Python package reads.

## What it edits

- **TAXONOMY** as a tree: groups (records and containers), numbers
  (`<primitive> <unit type>`), booleans, categoricals (ordered literals),
  derived categoricals (literals defined by expressions on other concepts),
  references to another type's id, and anything else as YAML.
- **MODULES** and **ODD** (the roots): `TITLE`, `DESCRIPTION`, `ACTIVE`,
  `LABEL(S)`, `METADATA`, one `INCLUDE_AND`/`INCLUDE_OR` and one
  `EXCLUDE_AND`/`EXCLUDE_OR` section, each with its one nested section of the
  other operator.
- **Conditions** by their form: true / false (a boolean, a module or a label),
  one of a categorical's literals (chips), a comparison (`<= 60 km/h`,
  `>= heavy`), a range (`[0 .. 40] km/h`), or any YAML value. The key offers
  every concept (by its dotted name or its own), module and label; one that
  names none is marked.
- **IMPORT**, and `conversion` / `COD` / `OD` kept as YAML.

Renaming a concept or a module renames the conditions that name it. Comments
are not kept: YAML readers drop them.

## The panel

- **YAML**: what the document writes. Edit it there and *Apply* to read it
  back into the forms.
- **Check**: what openodd makes of it, or why it cannot read it -- the name,
  the revision (the digest of the transmission, [versioning.md](versioning.md)),
  the declared version, the roots, every attribute's buckets with those outside
  the ODD in red, and every module's conditions as the reader normalised them.
  To bucket every attribute, the check measures each concept in the unit its
  conditions are first written in, as a binding file would.
- **Evaluate**: values a run would measure (empty: OpenODD's missing value),
  judged against the ODD -- inside, outside or inside only because values are
  missing, each module's verdict, and the derived categoricals' values.

The files a document `IMPORT`s can be loaded on the *Document* page; the check
reads them as a transmission, by name, as the Python package does.

The draft is kept in the browser's local storage between visits. *Download
YAML* saves the file.

## Building

```bash
rustup target add wasm32-unknown-unknown
cargo install trunk                 # or a release binary
cd crates/openodd-editor
trunk serve                         # http://127.0.0.1:8080, rebuilt on change
trunk build --release --cargo-profile wasm-release   # into dist/
```

The `wasm-release` profile (in the workspace `Cargo.toml`) optimises for size:
the page is about 1.6 MB of WebAssembly, 650 KB gzipped.

The model -- reading YAML into the forms' document and writing it back
(`doc.rs`), and the check and the evaluator (`check.rs`) -- is plain Rust, tested
natively with `cargo test -p openodd-editor`.

## The demo site

`.github/workflows/pages.yml` builds the editor and the
[coverage viewer](wasm.md) (under `viewer/`) on every push to `main`, and
deploys them to GitHub Pages. Pages must be set once to deploy from GitHub
Actions (*Settings → Pages → Build and deployment → Source: GitHub Actions*).
