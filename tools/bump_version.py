#!/usr/bin/env python3
"""Bumps the release version in every place that has to agree on it.

The version lives in two files: `Cargo.toml`'s `[package]` (what the crate carries,
and -- because `pyproject.toml` declares the version `dynamic` -- what maturin hands
PyPI) and `Cargo.lock` (the `openodd` entry, which `cargo --locked` would otherwise
reject). Editing them by hand is how they drift, so the auto-release job calls this
instead.

    tools/bump_version.py patch          # 0.1.0 -> 0.1.1
    tools/bump_version.py minor          # 0.1.0 -> 0.2.0
    tools/bump_version.py major          # 0.1.0 -> 1.0.0
    tools/bump_version.py 1.4.2          # set exactly
    tools/bump_version.py --current      # print the version, change nothing

The single line printed to stdout is the new version, which the workflow reads back to
form the tag. Nothing else is printed there, so it can be captured directly.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CARGO_TOML = ROOT / "Cargo.toml"
CARGO_LOCK = ROOT / "Cargo.lock"


def crate_name() -> str:
    text = CARGO_TOML.read_text()
    match = re.search(r'(?m)^\[package\][^\[]*?^name = "([^"]+)"', text)
    if match is None:
        raise SystemExit("could not find [package] name in Cargo.toml")
    return match.group(1)


def current_version() -> str:
    text = CARGO_TOML.read_text()
    match = re.search(r'(?m)^\[package\][^\[]*?^version = "([^"]+)"', text)
    if match is None:
        raise SystemExit("could not find [package] version in Cargo.toml")
    return match.group(1)


def next_version(current: str, spec: str) -> str:
    if spec not in {"major", "minor", "patch"}:
        if not re.fullmatch(r"\d+\.\d+\.\d+", spec):
            raise SystemExit(f"expected major|minor|patch or an X.Y.Z version, got {spec!r}")
        return spec
    major, minor, patch = (int(part) for part in current.split("."))
    if spec == "major":
        return f"{major + 1}.0.0"
    if spec == "minor":
        return f"{major}.{minor + 1}.0"
    return f"{major}.{minor}.{patch + 1}"


def set_cargo_toml(version: str) -> None:
    text = CARGO_TOML.read_text()
    text, count = re.subn(
        r'(?m)(^\[package\][^\[]*?^version = )"[^"]+"',
        rf'\g<1>"{version}"',
        text,
        count=1,
    )
    if count != 1:
        raise SystemExit("failed to rewrite [package] version in Cargo.toml")
    CARGO_TOML.write_text(text)


def set_cargo_lock(version: str) -> None:
    text = CARGO_LOCK.read_text()
    crate = crate_name()
    text, count = re.subn(
        rf'(\[\[package\]\]\nname = "{re.escape(crate)}"\nversion = )"[^"]+"',
        rf'\g<1>"{version}"',
        text,
        count=1,
    )
    if count != 1:
        raise SystemExit(f"failed to rewrite {crate} version in Cargo.lock")
    CARGO_LOCK.write_text(text)


def main() -> None:
    if len(sys.argv) != 2:
        raise SystemExit(__doc__)
    if sys.argv[1] == "--current":
        print(current_version())
        return
    version = next_version(current_version(), sys.argv[1])
    set_cargo_toml(version)
    set_cargo_lock(version)
    print(version)


if __name__ == "__main__":
    main()
