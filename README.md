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
