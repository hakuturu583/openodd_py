# openodd_py

[ASAM OpenODD 1.0](https://www.asam.net/standards/detail/openodd/) for Python, on a Rust core.

- Read an ODD from OpenODD YAML (taxonomy, modules, `IMPORT`, unit conversions) and a
  *binding* file saying how each concept is measured.
- Judge values against it with OpenODD's three-valued, missing-value semantics, and work
  out which buckets of each attribute lie outside it.
- **Version** it: an ODD's revision is the digest of the transmission it was read from
  (see [docs/versioning.md](docs/versioning.md)), and the transmission packs into a
  deterministic archive the ODD reads back from exactly.

- **Cover** what runs drive: `CoverageCollector` samples cover items and the ODD's
  attributes step by step (hits, seconds, metres, entries, `min_stay`, situations,
  steps outside the ODD) and writes a coverage file; `merge_coverage` /
  `openodd-coverage` merge any number into a report, and `--export-cod` writes each
  run's ODD samples as an OpenODD COD table (section 8.3) with its manifest and
  taxonomy.
- **Sample** the next runs from the ODD (`OddSampler`), aimed at what coverage
  left uncovered.
- **Find flaky cases**: `FlakySearch` learns where runs pass and fail (Gaussian-process
  classification in the Rust core) and draws part of each batch where a small change
  flips the outcome, keeping the sampler's own policy for the rest. Each case gets
  `flip_probability`, `instability_confidence` and `robustness_radius`
  ([docs/flaky.md](docs/flaky.md)). A run's coverage file can record its case, the
  predicted flakiness and its outcome; the merged report reassesses them, and the next
  campaign's search starts from them.
- **See it**: `CoverageView` renders an ODD matrix -- any two attributes, coloured
  flaky / fails / passes / outside the ODD / not sampled -- that drills into a cell
  for the next two, as an HTML component other tools embed or a standalone page
  (`openodd-coverage --html`); the `openodd-coverage-view` crate
  ([docs/coverage_view.md](docs/coverage_view.md)).
- **See the drawn cases**: `CasePlot` places every case of a sampled sweep at the
  values it was drawn with -- any two attributes of its space, ODD bucket edges as
  grid lines -- as a pass dot, a fail cross or a flaky ring with `passes/runs`,
  over a faint background of the pass probability the flaky model fitted to the
  runs predicts there (red low, yellow ~50% / unstable, green high), with a table of the cases run more than once; embeddable like `CoverageView`, or a page (`openodd-coverage
  --cases-html`) ([docs/case_plot.md](docs/case_plot.md)).
- **Drawn vs measured**: the case plot can place cases by what the run *measured* at its
  start instead of what the sampler *drew*, and the scenario-realisation view shows,
  per attribute, how far the scenario staged each run from its draw -- bias, MAE,
  bucket hit rate, and the knob correction a least-squares fit suggests
  (`CasePlot.drift_html`, `openodd-coverage --drift-html`;
  [docs/drift.md](docs/drift.md)).
- **In the browser**: both views compile to WebAssembly (`crates/openodd-wasm`): a web
  page computes the matrix and the case plot -- the flaky model's background included --
  from coverage JSON on the client, with live axis pickers; no Python, no server
  ([docs/wasm.md](docs/wasm.md), demo in [examples/wasm-viewer](examples/wasm-viewer)).
- **Comparing two sets on one ODD**: the same views with two sets of runs laid over one
  another -- a baseline and, say, the next version of the software -- each cell and each
  case split in two and outlined by how its failure rate moved, behind the cases the
  change of the predicted pass rate ([docs/compare.md](docs/compare.md)).
- **Test report**: `openodd-test-report runs/ -o report.html` lays a campaign's coverage
  files out as one HTML page -- verdict, results per scenario, every failed run, ODD
  coverage, flakiness and the interactive views -- with an analysis Claude writes through
  the Claude Agent SDK. Claude reads the report only through tools of its own, and
  follows the `odd-test-analysis` skill, which ships in the package as a Claude Code
  plugin along with the prompts (`pip install "openodd-py[agent]"`; `TestReport`, `openodd.agent.analyze`;
  [docs/test_report.md](docs/test_report.md)).
- **Find flaky regions of the ODD**: the merged report counts, per ODD cell, the runs
  that drove through it, how many failed (with a 95% Wilson interval) and how many
  failed *in* it, per scenario and environment (`CoverageReport.cell_flakiness`,
  [docs/flaky.md](docs/flaky.md#flakiness-per-odd-cell)).

- **Write an ODD in Python** instead of YAML (`OddDefinition`, `OddAttribute`,
  `OddModule`), register it by name (`register_odd`, the `openodd.odds` entry point
  group), and let users pick one by a string -- a bundled or registered name, a `.yaml`
  file or `package.module:function` (`resolve_odd`;
  [docs/python_odd.md](docs/python_odd.md)).

What measures the attributes -- a simulator, a recorded log -- is the application's
*backend*: a subclass of `openodd.Backend` naming its probes, how a run sets an
attribute (`knob`), and where the ego is (`position`, `geolocation`). Everything
else is this library's.

A `default` ODD ships with the package (`openodd.bundled_odd()`,
[python/openodd/odds/default](python/openodd/odds/default)): every attribute a road-vehicle
simulator commonly measures, after ISO 34503, and no modules. Its binding names the probes
a backend provides (`ego_speed_kph`, `measure: vehicle_ahead_gap_m`, ...); its revision
changes only when its files do.

```python
import openodd

backend = openodd.Backend(probes={"speed_kph": (read_speed, "km/h")})
odd = openodd.load_odd_binding("urban.yaml", backend=backend)
verdict = odd.evaluate(odd.sample(context))
odd.revision   # sha256 of the files it was read from
odd.archive()  # those files as a deterministic .tar.gz

collector = openodd.CoverageCollector(items, odd=odd, backend=backend)
collector.start(context, 0.0)
collector.tick(context, 0.05)  # after every step
collector.end(context, 0.05)
collector.write(Path("run_coverage.json"), "my_scenario")
```

### Coverage

A coverage document records, besides each item's buckets, how a run spent its
time across the ODD's buckets: `OddCells` adds up steps, seconds and metres per
*cell* -- one bucket of each attribute, as its cover item buckets it -- so a
consumer can slice runs by any attributes without the per-step values:

```python
cells = openodd.OddCells(odd)
cells.add(values, seconds=dt, meters=ds)   # every step: each attribute's value
cells.to_dict()
# {"attributes": ["speed", "rain"], "rows": [[["[30, 60]", "heavy"], 812, 40.6, 588.2], ...]}
```

A missing value is `<missing>`, a number outside the buckets `<below>` / `<above>`.
Each item's definition (`describe()`) names the measure its coverage is read from
(`amount`: `hits`, `counted.seconds`, ...) and, for an ODD attribute's item, the
attribute (`attribute`). `openodd.__version__` is the installed version, for
coverage files to say what wrote them.

## Development

```bash
uv venv && uv pip install maturin pytest hypothesis pyyaml
maturin develop
cargo test --workspace && pytest
```

## Releasing

Releases are cut by labels, as in simple_lanelet2. Put exactly one of
`release:major`, `release:minor` or `release:patch` on a PR; when it is merged into
`main`, the [release workflow](.github/workflows/release.yml) bumps the version in
`Cargo.toml` and `Cargo.lock` with `tools/bump_version.py`, commits
`chore(release): vX.Y.Z` and tags it, runs the tests again, builds the wheels (Linux
x86_64/aarch64, macOS universal2, Windows x64) and the sdist, tests each wheel, and attaches them to a
GitHub release with generated notes, and publishes them to PyPI with Trusted
Publishing. A PR without a label releases nothing. The
[release label check](.github/workflows/release-label.yml) runs on every PR: it creates any missing `release:*` label, fails if the PR carries more than one, and
says in its summary which release the merge will cut.

A hand-pushed `v*` tag is built and published the same way (its version must match
`Cargo.toml`), and running the workflow by hand builds everything without publishing.
