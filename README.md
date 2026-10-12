# openodd_py

[ASAM OpenODD 1.0](https://www.asam.net/standards/detail/openodd/) for Python, on a Rust core.

- Read an ODD from OpenODD YAML (taxonomy, modules, `IMPORT`, unit conversions) and a
  *binding* file saying how each concept is measured.
- Judge values against it with OpenODD's three-valued, missing-value semantics, and work
  out which buckets of each attribute lie outside it.
- **Version** it: an ODD's revision is the digest of the transmission it was read from
  (see [docs/versioning.md](docs/versioning.md)), and the transmission packs into a
  deterministic archive the ODD reads back from exactly.

What measures the attributes -- a simulator, a recorded log -- is the application's
*backend*; the core only judges values.

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
cargo test && pytest
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
