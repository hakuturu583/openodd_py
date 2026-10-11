"""Probes for the differential test: ``difftest_probes:read__<concept>`` reads
``context[<concept>]``, so one binding file works with any implementation."""

from typing import Any


def __getattr__(name: str) -> Any:
    if not name.startswith("read__"):
        raise AttributeError(name)
    key = name[len("read__"):]

    def probe(context: Any) -> Any:
        return context.get(key)

    probe.__name__ = probe.__qualname__ = name
    return probe
