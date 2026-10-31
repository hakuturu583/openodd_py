"""Fuzzy rules: how a backend derives what a run sets from what it was given.

A backend's knobs set what an ODD names -- the rain, the sun. What those do
to the rest of a run -- the road's grip, how far drivers see -- is the
backend's own model, which it may write as fuzzy rules in YAML (docs/rules.md)::

    version: 1
    quantities:
      tire_friction:
        rule_bases:
          water:
            inputs:
              water:
                universe: [0, 100]
                terms:
                  dry: {trapezoid: [0, 0, 0, 50]}
                  wet: {triangle: [0, 50, 100]}
                  flooded: {trapezoid: [50, 100, 100, 100]}
            rules:
              - if: (water IS dry)
                then: {low: 0.90, high: 1.10}
              - if: (water IS wet)
                then: {low: 0.65, high: 0.85}
              - if: (water IS flooded)
                then: {low: 0.45, high: 0.65}

Inference is Sugeno's, zero order, with simpful's operators (``AND`` the
least, ``OR`` the greatest, ``NOT`` the complement), in the Rust core. A
backend holds its rules (:attr:`~.backend.Backend.rules`), and a run may read
another file over them (:meth:`~.backend.Backend.use_rules`).

A rule file may ``extends`` others -- a path relative to it, an ``https://``
URL (``{url, sha256}`` pins its content), or ``{git, rev, path}`` -- and
writes only what it changes over them::

    version: 1
    extends: https://example.com/rules/base_weather.yaml
    quantities:
      tire_friction:
        rule_bases:
          water:
            rules: [...]          # its inputs are base_weather.yaml's
"""

from __future__ import annotations

import os
import urllib.request
from collections.abc import Callable, Mapping, Sequence
from pathlib import Path
from typing import Optional, Union

from . import _core

__all__ = ["Rules", "RulesError", "fetch_url"]

#: The most a rule file read from a URL may hold.
MAX_URL_BYTES = 4 * 1024 * 1024
#: Seconds a URL has to answer.
URL_TIMEOUT = 30.0


def fetch_url(url: str) -> str:
    """The text at an ``https://`` *url* (UTF-8, at most :data:`MAX_URL_BYTES`).

    Raises:
        RulesError: When it is not ``https://``, cannot be read, or is too large.
    """
    if not url.startswith("https://"):
        raise RulesError(f"{url}: only https:// URLs are read")
    try:
        with urllib.request.urlopen(url, timeout=URL_TIMEOUT) as response:
            data = response.read(MAX_URL_BYTES + 1)
    except OSError as e:
        raise RulesError(f"cannot read rules {url}: {e}") from e
    if len(data) > MAX_URL_BYTES:
        raise RulesError(f"cannot read rules {url}: larger than {MAX_URL_BYTES} bytes")
    return data.decode("utf-8")


class RulesError(ValueError):
    """A rule file that cannot be read, or inputs its rules say nothing of."""


class Rules:
    """A rule set: quantities, each rule bases whose outputs ``combine`` joins."""

    def __init__(self, core: "_core.RuleSet") -> None:
        self._core = core

    @classmethod
    def from_text(
        cls,
        text: str,
        source: str = "<rules>",
        base: Union[str, "os.PathLike[str]", None] = None,
        fetch: Optional[Callable[[str], str]] = None,
    ) -> "Rules":
        """The rules of a rule file's *text*; *source* names it in messages.
        What it ``extends`` is read relative to *base* (its path or URL; the
        working directory if not given), a URL with *fetch* (:func:`fetch_url`
        by default).

        Raises:
            RulesError: When the rules, or a file they extend, cannot be read
                or are not well formed.
        """
        where = None if base is None else str(base)
        return cls(cls._call(_core.RuleSet.parse, text, source, where, fetch or fetch_url))

    @classmethod
    def read(
        cls,
        location: Union[str, "os.PathLike[str]"],
        fetch: Optional[Callable[[str], str]] = None,
    ) -> "Rules":
        """The rules of the file at *location* -- a path, or an ``https://``
        URL read with *fetch* (:func:`fetch_url` by default) -- and the files
        it extends.

        Raises:
            RulesError: When it, or a file it extends, cannot be read or its
                rules are not well formed.
        """
        where = str(location)
        if not where.startswith(("https://", "http://")):
            where = str(Path(where))
        return cls(cls._call(_core.RuleSet.load, where, fetch or fetch_url))

    def read_over(
        self,
        location: Union[str, "os.PathLike[str]"],
        fetch: Optional[Callable[[str], str]] = None,
    ) -> "Rules":
        """The file at *location* (a path or an ``https://`` URL), and the
        files it extends, merged over these rules (:meth:`merged`). It need
        only name what it changes -- a rule base's ``rules`` without its
        ``inputs`` -- since it is checked once merged.

        Raises:
            RulesError: When it cannot be read or the merged rules are not
                well formed.
        """
        where = str(location)
        if not where.startswith(("https://", "http://")):
            where = str(Path(where))
        return Rules(self._call(self._core.load_over, where, fetch or fetch_url))

    def text_over(
        self,
        text: str,
        source: str = "<rules>",
        base: Union[str, "os.PathLike[str]", None] = None,
        fetch: Optional[Callable[[str], str]] = None,
    ) -> "Rules":
        """:meth:`read_over` for a rule file's *text* in hand."""
        where = None if base is None else str(base)
        return Rules(
            self._call(self._core.parse_over, text, source, where, fetch or fetch_url)
        )

    def merged(self, over: "Rules") -> "Rules":
        """These rules with *over*'s merged over them: a quantity, a rule base
        and an input *over* names replaces these rules' of that name, field by
        field down to the rule base (``null`` removes one), and a rule base's
        ``rules`` are replaced whole.

        Raises:
            RulesError: When the merged rules are not well formed.
        """
        return Rules(self._call(self._core.merged, over._core))

    @property
    def files(self) -> list[tuple[str, str]]:
        """``(path or URL, sha256)`` of every file read, the files extended
        first: what a run records of the rules it followed."""
        return self._core.files

    @property
    def quantities(self) -> list[str]:
        return self._core.quantities

    def inputs(self, quantity: str) -> list[str]:
        """The inputs *quantity*'s rule bases read."""
        return self._call(self._core.inputs, quantity)

    def outputs(self, quantity: str) -> list[str]:
        return self._call(self._core.outputs, quantity)

    def source(self, quantity: str) -> str:
        """The file *quantity*'s rules were read from."""
        return self._call(self._core.source, quantity)

    def evaluate(self, quantity: str, inputs: Mapping[str, float]) -> dict[str, float]:
        """Each output of *quantity* at *inputs*; an input not given reads its
        ``default``, and one outside its universe is clamped to it.

        Raises:
            RulesError: On an unknown quantity, an input not given that has no
                default, or inputs no rule fires at.
        """
        values = {str(k): float(v) for k, v in inputs.items()}
        return self._call(self._core.evaluate, quantity, values)

    def range(self, quantity: str, inputs: Mapping[str, float]) -> tuple[float, float]:
        """``(low, high)`` of a quantity whose outputs are ``low`` and ``high``."""
        out = self.evaluate(quantity, inputs)
        if "low" not in out or "high" not in out:
            raise RulesError(
                f"{self.source(quantity)}: {quantity} gives {sorted(out)}, not low and high"
            )
        return out["low"], out["high"]

    def check_inputs(self, allowed: Sequence[str]) -> None:
        """Refuse a rule base reading an input not in *allowed*."""
        self._call(self._core.check_inputs, [str(a) for a in allowed])

    @staticmethod
    def _call(f, *args):  # type: ignore[no-untyped-def]
        try:
            return f(*args)
        except ValueError as e:
            raise RulesError(str(e)) from None

    def __repr__(self) -> str:
        return f"Rules({', '.join(self.quantities)})"
