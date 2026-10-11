"""Read an ODD from ASAM OpenODD 1.0 YAML and a binding file.

The Rust core reads the YAML and builds the ODD; it records, for each
concept, which probe the binding names (``probe: speed_limit_kph``,
``measure: vehicle_ahead_gap_m``, ...). This module hands it the backend that
says what those names are -- their units while reading, their callables
afterwards -- so the same binding file can be measured by a simulator, a log
reader or nothing at all.
"""

from __future__ import annotations

import json
import os
from collections.abc import Mapping
from pathlib import Path
from typing import Any, Optional, Union

from . import _core
from .backend import Backend, default_backend
from .model import OddDefinition, Probe, _missing

__all__ = [
    "GitSource",
    "OpenOddError",
    "load_odd_archive",
    "load_odd_binding",
    "load_odd_file",
    "load_openodd",
    "read_archive_manifest",
]

OpenOddError = _core.OpenOddError
GitSourceError = _core.GitSourceError


class GitSource(tuple):
    """OpenODD files in a git repository at a revision: ``(url, rev, path)``."""

    def __new__(cls, url: str, rev: str, path: str) -> "GitSource":
        return super().__new__(cls, (url, rev, path))

    @property
    def url(self) -> str:
        return self[0]

    @property
    def rev(self) -> str:
        return self[1]

    @property
    def path(self) -> str:
        return self[2]


def _probes(core: Any, backend: Backend) -> dict[str, Probe]:
    """The callables the backend names for the ODD's attributes."""
    probes: dict[str, Probe] = {}
    for attribute in core.attributes:
        kind, spec = attribute.probe
        if kind == "probe":
            probes[attribute.name] = backend.probe(spec)[0]
        elif kind in ("missing", "derived", "code"):
            probes[attribute.name] = _missing
        else:
            probes[attribute.name] = backend.read(kind, spec)[0]
    return probes


def _wrap(core: Any, backend: Backend) -> OddDefinition:
    return OddDefinition._from_core(core, _probes(core, backend))


def load_odd_binding(
    path: Union[str, os.PathLike[str]], *, backend: Optional[Backend] = None
) -> OddDefinition:
    """Read an ODD from a binding file: OpenODD files, and how to measure them.

    A binding file is this library's, not OpenODD's::

        openodd:                             # relative to this file,
          - odd.yaml
          - git: https://example.com/odd/taxonomy.git
            rev: v1.2.0                      # or in git
            path: taxonomy.yaml
        name: urban                          # default: this file's stem
        text: Urban roads, fair weather
        probes:
          speed_limit:                       # a concept, as conditions name it
            probe: speed_limit_kph           # the backend's, or package.module:function
            unit: km/h                       # what the probe returns
            buckets: [0, 30, 60, 90]         # or values, or range + every
            cover_by: meters                 # optional: hits, seconds, meters, entries
            target: 200                      # optional
            min_stay: 2                      # optional
        situations:
          close_cut_in: {target: 1}
    """
    backend = backend or default_backend()
    return _wrap(_core.load_binding(Path(path), backend), backend)


def load_odd_file(
    path: Union[str, os.PathLike[str]], *, backend: Optional[Backend] = None
) -> OddDefinition:
    """Read a ``.yaml`` ODD file: a binding file, or an OpenODD document on its
    own (read with no probes, so nothing is measured)."""
    backend = backend or default_backend()
    return _wrap(_core.load_file(Path(path), backend), backend)


def load_openodd(
    *sources: Union[str, os.PathLike[str], GitSource],
    bindings: Optional[Mapping[str, Any]] = None,
    name: Optional[str] = None,
    text: str = "",
    situations: Optional[Mapping[str, Any]] = None,
    backend: Optional[Backend] = None,
) -> OddDefinition:
    """Read an ODD from OpenODD YAML files or text.

    Args:
        sources: Paths, YAML text, or files in git (:class:`GitSource`), read
            together as if one imported the others.
        bindings: Concept -> how to measure it (a binding file's ``probes``).
        name: The ODD's name; the first file's stem by default.
        text: A description for the report.
        situations: Module -> its criteria: the modules to cover as situations.
        backend: Whose probes the bindings name (the default backend if none).
    """
    if not sources:
        raise OpenOddError("load_openodd(): no sources")
    backend = backend or default_backend()
    items: list[Any] = []
    for source in sources:
        if isinstance(source, GitSource):
            items.append(tuple(source))
        elif isinstance(source, os.PathLike):
            items.append(Path(source))
        else:
            items.append(str(source))
    core = _core.load_openodd(
        items,
        backend,
        bindings=None if bindings is None else json.dumps(_plain(bindings)),
        name=name,
        text=text,
        situations=None if situations is None else json.dumps(_plain(situations)),
    )
    return _wrap(core, backend)


def load_odd_archive(archive: bytes, *, backend: Optional[Backend] = None) -> OddDefinition:
    """Read an ODD back from its transmission archive (:meth:`OddDefinition.archive`)."""
    backend = backend or default_backend()
    return _wrap(_core.load_archive(archive, backend), backend)


def read_archive_manifest(archive: bytes) -> dict[str, Any]:
    """A transmission archive's manifest: its revision, version, entry and files."""
    return json.loads(_core.archive_manifest(archive))


def _plain(value: Any) -> Any:
    """Bindings as JSON (YAML's subset the core reads): callables by name."""
    if isinstance(value, Mapping):
        return {str(k): _plain(v) for k, v in value.items()}
    if isinstance(value, (list, tuple)):
        return [_plain(v) for v in value]
    if callable(value):
        module = getattr(value, "__module__", "")
        return f"{module}:{getattr(value, '__qualname__', '')}"
    return value
