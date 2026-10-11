"""OpenODD files kept in git repositories, pinned to a revision.

Each repository is fetched once into a cache (``$OPENODD_CACHE``, or
``~/.cache/openodd``) and each commit checked out once: the files of a commit
never change, so a commit already checked out is used without the network.
"""

from __future__ import annotations

from pathlib import Path
from typing import NamedTuple

from . import _core
from .loader import GitSource

__all__ = ["CACHE_ENV", "Checkout", "GitSource", "GitSourceError", "cache_dir", "fetch"]

CACHE_ENV: str = _core.CACHE_ENV
GitSourceError = _core.GitSourceError


class Checkout(NamedTuple):
    """A commit's files in the cache."""

    root: Path
    commit: str


def fetch(url: str, rev: str) -> Checkout:
    """The files of *url* at *rev* (a tag, a branch or a commit), fetched if need be."""
    root, commit = _core.fetch(url, rev)
    return Checkout(Path(root), commit)


def cache_dir() -> Path:
    """Where repositories and checkouts are cached."""
    return Path(_core.cache_dir())
