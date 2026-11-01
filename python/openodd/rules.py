"""Rules: how a backend derives what a run sets from what it was given.

A backend's knobs set what an ODD names -- the rain, the sun. What those do
to the rest of a run -- the road's grip, how far drivers see -- is the
backend's own model, which it writes as rules in YAML (docs/rules.md): each a
conditional distribution, the distribution of a quantity given some inputs::

    version: 2
    quantities:
      tire_friction:
        parts:
          water:
            at: {water: [0, 50, 100]}      # the input, and its points
            uniform:                       # the distribution at each point
              low:  [0.90, 0.65, 0.45]
              high: [1.10, 0.85, 0.65]

Each parameter is interpolated linearly between the points and held beyond
them; a distribution is ``uniform`` (``low``, ``high``), ``triangular``
(``low``, ``mode``, ``high``) or ``normal`` (``mean``, ``sd``, truncated to
``low``..``high``). A run draws a value at a quantile (:meth:`Rules.quantile`).
With several parts, ``combine`` (``min``, the default, or ``max``) joins the
values they give at the same quantile. The Rust core reads and infers them.
A backend holds its rules (:attr:`~.backend.Backend.rules`), and a run may
read another file over them (:meth:`~.backend.Backend.use_rules`).

A rule file may ``extends`` others -- a path relative to it, an ``https://``
URL (``{url, sha256}`` pins its content), or ``{git, rev, path}`` -- and
writes only what it changes over them::

    version: 2
    extends: https://example.com/rules/base_weather.yaml
    quantities:
      tire_friction:
        parts:
          water:
            uniform: {low: [0.95, 0.60, 0.30], high: [1.05, 0.80, 0.50]}
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
    """A rule set: quantities, each parts whose values ``combine`` joins."""

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
        only name what it changes -- a part's distribution without its
        ``at`` -- since it is checked once merged.

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
        """These rules with *over*'s merged over them: a quantity, a part and
        an input *over* names merges over these rules' of that name, field by
        field (``null`` removes one); a part's ``at`` and its distribution are
        replaced whole.

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
        """The inputs *quantity*'s parts read."""
        return self._call(self._core.inputs, quantity)

    @property
    def input_defaults(self) -> dict[str, Optional[float]]:
        """The inputs the file describes, and the value each reads when not
        given (``None``: none)."""
        return dict(self._core.input_defaults())

    def source(self, quantity: str) -> str:
        """The file *quantity*'s rules were read from."""
        return self._call(self._core.source, quantity)

    def combine(self, quantity: str) -> str:
        """``min`` or ``max``: how *quantity*'s parts are joined."""
        return self._call(self._core.combine, quantity)

    def evaluate(self, quantity: str, inputs: Mapping[str, float]) -> dict[str, float]:
        """``low``, ``high``, ``median`` and ``mean`` of *quantity* at
        *inputs*; an input not given reads its ``default``, and one beyond its
        outermost points is held there.

        Raises:
            RulesError: On an unknown quantity, or an input not given that has
                no default.
        """
        return self._call(self._core.evaluate, quantity, _values(inputs))

    def range(self, quantity: str, inputs: Mapping[str, float]) -> tuple[float, float]:
        """``(low, high)``: the least and greatest value *quantity* takes at
        *inputs*."""
        out = self.evaluate(quantity, inputs)
        return out["low"], out["high"]

    def quantile(self, quantity: str, inputs: Mapping[str, float], p: float) -> float:
        """The value of *quantity* at quantile *p* (0 to 1, clamped) at
        *inputs*: a share *p* of its mass lies below it. A run draws *p* and
        reads the value here; with several parts, each gives its value at
        *p* and ``combine`` joins them."""
        return self._call(self._core.quantile, quantity, _values(inputs), float(p))

    def cdf(self, quantity: str, inputs: Mapping[str, float], x: float) -> float:
        """The share of *quantity*'s mass at or below *x*, at *inputs*."""
        return self._call(self._core.cdf, quantity, _values(inputs), float(x))

    def distributions(
        self, quantity: str, inputs: Mapping[str, float]
    ) -> list[tuple[str, str, dict[str, float]]]:
        """Each part's distribution at *inputs*: ``(part, family, params)``."""
        return [
            (name, family, dict(params))
            for name, family, params in self._call(
                self._core.distributions, quantity, _values(inputs)
            )
        ]

    def check_inputs(self, allowed: Sequence[str]) -> None:
        """Refuse a part reading an input not in *allowed*."""
        self._call(self._core.check_inputs, [str(a) for a in allowed])

    @staticmethod
    def _call(f, *args):  # type: ignore[no-untyped-def]
        try:
            return f(*args)
        except ValueError as e:
            raise RulesError(str(e)) from None

    def __repr__(self) -> str:
        return f"Rules({', '.join(self.quantities)})"


def _values(inputs: Mapping[str, float]) -> dict[str, float]:
    return {str(k): float(v) for k, v in inputs.items()}
