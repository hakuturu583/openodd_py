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

```python
import openodd

backend = openodd.Backend(probes={"speed_kph": (read_speed, "km/h")})
odd = openodd.load_odd_binding("urban.yaml", backend=backend)
verdict = odd.evaluate(odd.sample(context))
odd.revision   # sha256 of the files it was read from
odd.archive()  # those files as a deterministic .tar.gz
```

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
x86_64/aarch64, macOS universal2) and the sdist, tests each wheel, and attaches them to a
GitHub release with generated notes. Publishing to PyPI (Trusted Publishing) also runs
once the repository variable `PUBLISH_PYPI` is `true`. A PR without a label releases
nothing.

A hand-pushed `v*` tag is built and published the same way (its version must match
`Cargo.toml`), and running the workflow by hand builds everything without publishing.
