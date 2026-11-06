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

A part may read another quantity, so rules form a graph (a chain of
conditional distributions)::

    quantities:
      visibility:
        parts:
          fog: {at: {fog_density: [0, 100]}, uniform: {low: [250, 15], high: [400, 40]}}
      sight_speed:
        parts:
          sight: {at: {visibility: [15, 400]}, uniform: {low: [5, 25], high: [10, 35]}}

A quantity read and not given is its value at its own quantile
(``quantiles``, the median by default); one given is taken as observed.
:attr:`Rules.order` lists the quantities each after those it reads, and
:meth:`Rules.sample` gives every one. A cycle is refused.
A backend holds its rules (:attr:`~.backend.Backend.rules`), and a run may
read another file over them (:meth:`~.backend.Backend.use_rules`).

Rules fit data: :meth:`Rules.fit` gives the maximum-likelihood estimate of
every part's parameters at its points from rows of values measured together
(a log of the rain and the grip a vehicle found in it), each quantity as a
factor of its own, and writes the fitted parts as a rule file over the rules
fitted (docs/rules.md, "Fitting to data")::

    fit = rules.fit(read_rows("drives.csv"))
    fit.quantities["grip"].log_likelihood
    fit.write("fitted.yaml", extends="grip.yaml")

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

import argparse
import datetime as _dt
import hashlib
import csv
import json
import os
import sys
import urllib.request
import warnings
from collections.abc import Callable, Iterable, Mapping, Sequence
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Optional, Union

from . import _core

__all__ = [
    "Calibration",
    "PartFit",
    "QuantityFit",
    "Rules",
    "RulesError",
    "RulesFit",
    "UnclassifiedInputsWarning",
    "fetch_url",
    "read_data",
    "read_rows",
]

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


class UnclassifiedInputsWarning(UserWarning):
    """Rules read inputs that stand for no taxonomy concept (a version 2 file):
    their causes cannot be traced to an ODD's vocabulary."""


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
        return cls._read(cls._call(_core.RuleSet.parse, text, source, where, fetch or fetch_url))

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
        return cls._read(cls._call(_core.RuleSet.load, where, fetch or fetch_url))

    @classmethod
    def _read(cls, core: "_core.RuleSet") -> "Rules":
        rules = cls(core)
        unclassified = rules.unclassified_inputs
        if unclassified:
            warnings.warn(
                f"{rules.files[-1][0] if rules.files else '<rules>'}: version {rules.version} rules "
                f"read inputs that stand for no taxonomy concept ({', '.join(unclassified)}); "
                "version 3 names each input's concept (docs/rules.md, \"Inputs and the taxonomy\")",
                UnclassifiedInputsWarning,
                stacklevel=3,
            )
        return rules

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

    def extended(self, added: "Rules") -> "Rules":
        """These rules with *added*'s nodes added: the network these rules are
        (a backend's, which every scenario shares) and the nodes a scenario
        adds to it. An added node may read these rules' quantities and inputs.
        Unlike :meth:`merged`, nothing is replaced, and each side keeps its
        own ``basis``.

        Raises:
            RulesError: When *added* defines a quantity these rules define,
                describes an input otherwise (its ``default``, ``concept`` or
                ``unit``), or draws on a taxonomy of other content; or when the
                joined rules are not well formed.
        """
        return Rules(self._call(self._core.extended, added._core))

    def read_extending(
        self,
        location: Union[str, "os.PathLike[str]"],
        fetch: Optional[Callable[[str], str]] = None,
    ) -> "Rules":
        """The file at *location* (a path or an ``https://`` URL), and the
        files it extends, added to these rules (:meth:`extended`). It is
        checked joined to them, so its nodes may read theirs: a version 3
        node that reads a shared quantity names no concept for it.

        Raises:
            RulesError: When it cannot be read, or :meth:`extended` refuses it.
        """
        where = str(location)
        if not where.startswith(("https://", "http://")):
            where = str(Path(where))
        return Rules(self._call(self._core.load_extending, where, fetch or fetch_url))

    def text_extending(
        self,
        text: str,
        source: str = "<rules>",
        base: Union[str, "os.PathLike[str]", None] = None,
        fetch: Optional[Callable[[str], str]] = None,
    ) -> "Rules":
        """:meth:`read_extending` for a rule file's *text* in hand."""
        where = None if base is None else str(base)
        return Rules(
            self._call(self._core.parse_extending, text, source, where, fetch or fetch_url)
        )

    @property
    def input_labels(self) -> dict[str, list[str]]:
        """Each categorical input's labels, in order: it is given as one (its
        parts are written at labels, and nothing lies between two)."""
        return dict(self._core.input_labels())

    def _inputs(self, inputs: Optional[Mapping[str, Any]]) -> dict[str, float]:
        """*inputs* as the core reads them: a categorical input's label as its
        index."""
        if not inputs:
            return {}
        labels = self._core.input_labels()
        out: dict[str, float] = {}
        for k, v in inputs.items():
            name = str(k)
            if name in labels and not (isinstance(v, (int, float)) and not isinstance(v, bool)):
                from .items import value_label  # noqa: PLC0415 - items imports this module's peers

                # A label as the core writes it: a bool in lowercase, an enum by name.
                label = value_label(v)
                try:
                    out[name] = float(labels[name].index(label))
                except ValueError:
                    raise RulesError(
                        f"{name} is {label!r}, not one of its labels {', '.join(labels[name])}"
                    ) from None
            else:
                out[name] = float(v)
        return out

    @property
    def version(self) -> int:
        """The format's version: the newest of the files read."""
        return self._core.version

    @property
    def taxonomy(self) -> Optional[tuple[str, str]]:
        """``(path or URL, sha256)`` of the taxonomy a version 3 set names."""
        return self._core.taxonomy

    @property
    def taxonomy_concepts(self) -> dict[str, dict[str, Any]]:
        """Concept -> what the taxonomy a version 3 set names declares it to
        be (``kind``, and ``primitive``, ``unit_type``, ``literals``), as
        :attr:`.OddAttribute.concept` gives it; empty for version 2. Kept
        from when the set was read: the taxonomy is not read again."""
        return dict(json.loads(self._core.taxonomy_concepts_json()))

    @property
    def input_concepts(self) -> dict[str, tuple[str, str]]:
        """Input -> ``(concept, unit)``: the taxonomy concept each input
        stands for (version 3), and the unit its points are written in."""
        return dict(self._core.input_concepts())

    @property
    def unclassified_inputs(self) -> list[str]:
        """The inputs the parts read that stand for no taxonomy concept: those
        of a version 2 set (version 3 refuses them)."""
        return list(self._core.unclassified_inputs())

    def parts(self, quantity: str) -> list[str]:
        """*quantity*'s parts, in the order written."""
        return list(self._call(self._core.parts, quantity))

    def part_axes(self, quantity: str, part: str) -> list[tuple[str, list[float]]]:
        """*part*'s inputs, in the order its ``at`` writes them, each with
        its points."""
        return [(n, list(p)) for n, p in self._call(self._core.part_axes, quantity, part)]

    def basis(self, quantity: str, part: str) -> dict[str, Any]:
        """What *part* of *quantity* rests on: ``kind`` (``fitted``,
        ``expert``, ``literature``, or ``unstated`` when nothing says) and
        what its ``basis:`` says besides -- the part's own, else its
        quantity's, else the file's. A rule is an assumption about how
        faithfully a run renders its conditions, not part of an ODD; this is
        what the assumption rests on.

        Raises:
            RulesError: When there is no such quantity or part.
        """
        return dict(json.loads(self._call(self._core.basis, quantity, part)))

    def bases(self) -> dict[tuple[str, str], dict[str, Any]]:
        """``(quantity, part)`` -> :meth:`basis`, for every part."""
        return {
            (q, part): self.basis(q, part)
            for q in self.quantities
            for part in self.parts(q)
        }

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

    def evaluate(
        self,
        quantity: str,
        inputs: Mapping[str, Any],
        quantiles: Optional[Mapping[str, float]] = None,
    ) -> dict[str, float]:
        """``low``, ``high``, ``median`` and ``mean`` of *quantity* at
        *inputs*; an input not given reads its ``default``, and one beyond its
        outermost points is held there. A quantity it reads that *inputs* does
        not give is at its quantile in *quantiles* (the median when not
        named).

        Raises:
            RulesError: On an unknown quantity, or an input not given that has
                no default.
        """
        return self._call(
            self._core.evaluate, quantity, self._inputs(inputs), _values(quantiles)
        )

    def range(
        self,
        quantity: str,
        inputs: Mapping[str, Any],
        quantiles: Optional[Mapping[str, float]] = None,
    ) -> tuple[float, float]:
        """``(low, high)``: the least and greatest value *quantity* takes at
        *inputs*."""
        out = self.evaluate(quantity, inputs, quantiles)
        return out["low"], out["high"]

    def quantile(
        self,
        quantity: str,
        inputs: Mapping[str, Any],
        p: float,
        quantiles: Optional[Mapping[str, float]] = None,
    ) -> float:
        """The value of *quantity* at quantile *p* (0 to 1, clamped) at
        *inputs*: a share *p* of its mass lies below it. A run draws *p* and
        reads the value here; with several parts, each gives its value at
        *p* and ``combine`` joins them. A quantity it reads is at its own
        quantile in *quantiles* (the median when not named)."""
        return self._call(
            self._core.quantile,
            quantity,
            self._inputs(inputs),
            float(p),
            _values(quantiles),
        )

    def cdf(
        self,
        quantity: str,
        inputs: Mapping[str, Any],
        x: float,
        quantiles: Optional[Mapping[str, float]] = None,
    ) -> float:
        """The share of *quantity*'s mass at or below *x*, at *inputs*."""
        return self._call(
            self._core.cdf, quantity, self._inputs(inputs), float(x), _values(quantiles)
        )

    def distributions(
        self,
        quantity: str,
        inputs: Mapping[str, Any],
        quantiles: Optional[Mapping[str, float]] = None,
    ) -> list[tuple[str, str, dict[str, float]]]:
        """Each part's distribution at *inputs*: ``(part, family, params)``."""
        return [
            (name, family, dict(params))
            for name, family, params in self._call(
                self._core.distributions,
                quantity,
                self._inputs(inputs),
                _values(quantiles),
            )
        ]

    @property
    def order(self) -> list[str]:
        """The quantities, each after the quantities its parts read."""
        return self._core.order()

    def upstream(self, quantity: str) -> list[str]:
        """The quantities *quantity*'s parts read."""
        return self._call(self._core.upstream, quantity)

    @property
    def free_inputs(self) -> list[str]:
        """The inputs the parts read that are not quantities: what a caller
        gives."""
        return self._core.free_inputs()

    def sample(
        self,
        inputs: Mapping[str, Any],
        quantiles: Optional[Mapping[str, float]] = None,
    ) -> dict[str, float]:
        """Every quantity's value at *inputs*, each at its own quantile in
        *quantiles* (0.5 when not named), in :attr:`order`: what one run
        sets. A quantity given in *inputs* keeps the value given."""
        return dict(
            self._call(self._core.sample, self._inputs(inputs), _values(quantiles))
        )

    def check_inputs(self, allowed: Sequence[str]) -> None:
        """Refuse a part reading an input not in *allowed*."""
        self._call(self._core.check_inputs, [str(a) for a in allowed])

    def log_likelihood(
        self,
        rows: Iterable[Mapping[str, Any]],
        weights: Optional[Sequence[float]] = None,
        *,
        weight_column: Optional[str] = None,
    ) -> dict[str, tuple[float, int]]:
        """The log-likelihood of *rows* under these rules, per quantity:
        ``(log-likelihood, rows)``, of the rows that give the quantity and
        every value its parts read (an input missing reads its ``default``).
        ``-inf`` when a row lies outside the quantity's range. A row is a
        mapping of names to values, a value ``None``, ``""`` or NaN not
        observed; *weights*, or the column *weight_column* names, weighs
        each row.

        Raises:
            RulesError: On a value that is not a number, or *weights* not one
                per row, or one negative.
        """
        data, weights = self._rows(rows, weights, weight_column)
        return dict(self._call(self._core.log_likelihood, data, weights))

    def calibration(
        self,
        rows: Iterable[Mapping[str, Any]],
        weights: Optional[Sequence[float]] = None,
        *,
        weight_column: Optional[str] = None,
    ) -> dict[str, "Calibration"]:
        """How well these rules match *rows*, per quantity some row gives
        with everything it reads (:class:`Calibration`): where each row's
        value falls in the quantity's distribution at its inputs. Rows and
        weights as for :meth:`log_likelihood`."""
        data, weights = self._rows(rows, weights, weight_column)
        return {
            name: Calibration(**c)
            for name, c in self._call(self._core.calibration, data, weights).items()
        }

    def fit(
        self,
        rows: Iterable[Mapping[str, Any]],
        weights: Optional[Sequence[float]] = None,
        *,
        weight_column: Optional[str] = None,
        quantities: Optional[Sequence[str]] = None,
        max_iterations: int = 1000,
        tolerance: float = 1e-10,
        min_spread: float = 1e-3,
        fixed: Sequence[str] = (),
        data_files: Sequence[Union[str, "os.PathLike[str]"]] = (),
        by: Optional[str] = None,
        date: Optional[str] = None,
    ) -> "RulesFit":
        """The maximum-likelihood fit of these rules to *rows*: data
        measured in the real world -- each row the values seen together, by
        name, as for :meth:`log_likelihood`.

        The rules are a factor graph, each quantity a conditional
        distribution given what its parts read, so the likelihood factors
        by quantity and each is fitted on its own, from the rows that give it
        and everything it reads. What is estimated is every parameter at
        every point a part writes -- a uniform's ``low`` and ``high``, a
        triangular's ``low``, ``mode`` and ``high``, a normal's ``mean`` and
        ``sd`` (its ``low``..``high``, the physical range, is held) -- from
        the rules' own values; a point no row reads keeps its value. The
        families and the points stay as written.

        What must not move is held as written: by the rule file (a part's
        ``fixed:``, docs/rules.md) or by *fixed* here.

        Args:
            rows: The data.
            weights: Each row's weight (1 when not given).
            weight_column: Instead of *weights*, the column each row's weight
                is in (1 where a row has none).
            quantities: The quantities to fit; by default every one some row
                gives with all it reads. One named that no row gives is an
                error.
            max_iterations: The most iterations of each stage of the fit.
            tolerance: Converged when an iteration raises the mean
                log-likelihood per row by less.
            min_spread: The least width of a distribution at a point with
                data (``high - low``, or a normal's spread as truncated --
                its interquartile range over 1.349), as a share of
                the standard deviation of the quantity's values (of their
                size, when they are all one): a likelihood grows without
                bound as a distribution narrows onto one value.
            fixed: Values held as written, besides those the rules' own
                ``fixed`` holds: ``"grip"`` (a quantity), ``"grip.water"``
                (a part), ``"grip.water.sd"`` (a parameter at every point),
                ``"grip.water.mean@water=0"`` (a parameter at a point: every
                input the part reads at one of its points) or
                ``"grip.water@water=0"`` (every parameter there). A quantity
                held whole is not fitted.
            data_files: The files *rows* were read from: each fitted part's
                ``basis`` names them with their SHA-256, besides the rows
                and the calibration after the fit (docs/rules.md, "The basis
                of a part"). A part held whole keeps the basis it has.
            by: Who fitted, for ``basis.by``: ``openodd <version>`` if not
                given.
            date: When, for ``basis.date``: today (UTC) if not given; ``""``
                writes none.

        Raises:
            RulesError: On data that is not numbers, a row outside a normal
                distribution's ``low``..``high``, a quantity named that no
                row gives, a ``fixed`` entry naming nothing, a row outside a
                range held, or options out of range.
        """
        if max_iterations < 0:
            raise RulesError(f"max_iterations must not be negative, not {max_iterations}")
        data, weights = self._rows(rows, weights, weight_column)
        core = self._call(
            self._core.fit,
            data,
            weights,
            None if quantities is None else [str(q) for q in quantities],
            int(max_iterations),
            float(tolerance),
            float(min_spread),
            [str(f) for f in fixed],
            [(str(f), _sha256_of(f)) for f in data_files],
            by,
            _dt.datetime.now(_dt.timezone.utc).date().isoformat() if date is None else date,
        )
        fitted = {
            q["name"]: QuantityFit(
                **{
                    **q,
                    "parts": tuple(PartFit(**p) for p in q["parts"]),
                    "calibration_before": Calibration(**q["calibration_before"]),
                    "calibration": Calibration(**q["calibration"]),
                }
            )
            for q in core.quantities
        }
        return RulesFit(rules=Rules(core.rules), quantities=fitted, _core=core)

    def _rows(
        self,
        rows: Iterable[Mapping[str, Any]],
        weights: Optional[Sequence[float]],
        weight_column: Optional[str],
    ) -> tuple[list[dict[str, float]], Optional[list[float]]]:
        """*rows* as the core reads them -- the values these rules read, as
        numbers -- and their weights, given or from *weight_column*."""
        if weights is not None and weight_column is not None:
            raise RulesError("give weights or weight_column, not both")
        data, from_column = self._call(
            self._core.observations, [dict(r) for r in rows], weight_column
        )
        return data, from_column if weights is None else [float(w) for w in weights]

    @staticmethod
    def _call(f, *args):  # type: ignore[no-untyped-def]
        try:
            return f(*args)
        except ValueError as e:
            raise RulesError(str(e)) from None

    def __repr__(self) -> str:
        return f"Rules({', '.join(self.quantities)})"


@dataclass(frozen=True)
class Calibration:
    """How well a quantity's distribution matches rows of data, from where
    each row's value falls in it: its quantile ``u = cdf(value)`` at the
    row's inputs. Under rules that match the data, ``u`` is spread evenly
    over 0..1, whatever the family. Shares are of the rows' weight."""

    #: The rows (giving the quantity and all it reads).
    rows: int
    #: The log-likelihood per unit of weight (``-inf``: a row is impossible).
    mean_log_likelihood: float
    #: The share of rows in the distribution's central 90%: 0.9 when it
    #: matches, less when too narrow or off-centre, more when too wide.
    in_band: float
    #: The share of rows the rules hold impossible.
    outside: float
    #: Kolmogorov-Smirnov's distance of ``u`` from even (about
    #: ``1.36 / sqrt(rows)`` is what chance alone gives 5% of the time).
    ks: float
    #: The share of rows in each tenth of the quantiles: flat when it
    #: matches, a U when too narrow, a hump when too wide.
    histogram: tuple[float, ...]

    def __post_init__(self) -> None:
        object.__setattr__(self, "histogram", tuple(self.histogram))


@dataclass(frozen=True)
class PartFit:
    """What one part's fit touched."""

    name: str
    #: The points the part writes.
    points: int
    #: The points some row reads.
    points_with_data: int
    #: The values estimated: each parameter fitted at each point with data,
    #: less those held (``fixed``).
    estimated: int


@dataclass(frozen=True)
class QuantityFit:
    """How one quantity was fitted."""

    name: str
    #: The rows that give it and every value it reads.
    rows: int
    #: The rows that give it but miss something it reads.
    skipped: int
    #: The (weighted) log-likelihood of those rows under the rules fitted:
    #: ``-inf`` when a row lies outside their range.
    log_likelihood_before: float
    #: ... and under the fitted rules.
    log_likelihood: float
    iterations: int
    #: Whether the fit converged, not stopped at ``max_iterations``.
    converged: bool
    parts: tuple[PartFit, ...]
    #: How well the rules as they were match the rows ...
    calibration_before: Calibration
    #: ... and the fitted rules.
    calibration: Calibration


@dataclass(frozen=True)
class RulesFit:
    """A fit (:meth:`Rules.fit`): the fitted rules, and how each quantity
    was fitted."""

    #: The rules fitted, every fitted part's distribution replaced.
    rules: Rules
    #: Per quantity fitted, in :attr:`Rules.order`.
    quantities: dict[str, QuantityFit]
    _core: "_core.RuleFit" = field(repr=False)

    @property
    def overlay(self) -> str:
        """The fitted parts as a rule file that merges over the rules fitted."""
        return self.text()

    def text(self, extends: Optional[str] = None) -> str:
        """The fitted parts as a rule file. With *extends* -- a reference to
        the rules fitted, as a rule file's ``extends`` takes it -- the file
        extends them, so :meth:`Rules.read` reads it as the whole fitted
        set; without, it merges over them (:meth:`Rules.read_over`)."""
        return self._core.text(None if extends is None else str(extends))

    def write(
        self,
        path: Union[str, "os.PathLike[str]"],
        extends: Union[str, "os.PathLike[str]", None] = None,
    ) -> None:
        """Write :meth:`text` to *path*. *extends*, the rules fitted -- a
        path (written relative to *path*'s directory) or a URL -- makes the
        file extend them."""
        path = Path(path)
        reference: Union[str, None] = None
        if extends is not None:
            reference = str(extends)
            if not _is_url(reference):
                reference = Path(
                    os.path.relpath(Path(reference).resolve(), path.parent.resolve())
                ).as_posix()
        path.write_text(self.text(reference), encoding="utf-8")


def _is_url(location: Union[str, "os.PathLike[str]"]) -> bool:
    return str(location).startswith(("https://", "http://"))


def read_rows(path: Union[str, "os.PathLike[str]"]) -> list[dict[str, Any]]:
    """The rows of a data file: CSV with a header (``.csv``, ``.tsv``),
    JSON lines (``.jsonl``, ``.ndjson``: an object per line), or JSON -- a
    list of objects, ``{"rows": [...], "weight": "<column>"}``, or an object
    of columns, each a list (the editor reads the same JSON). Values are left
    as read; :meth:`Rules.fit` reads them as numbers.

    Raises:
        RulesError: When the file cannot be read or is none of these.
    """
    return read_data(path)[0]


def read_data(path: Union[str, "os.PathLike[str]"]) -> tuple[list[dict[str, Any]], Optional[str]]:
    """:func:`read_rows`, and the weight column a JSON file names
    (``"weight"``; ``None`` when it names none)."""
    path = Path(path)
    weight: Optional[str] = None
    suffix = path.suffix.lower()
    try:
        text = path.read_text(encoding="utf-8-sig")
    except OSError as e:
        raise RulesError(f"cannot read {path}: {e}") from None
    try:
        if suffix in (".csv", ".tsv"):
            reader = csv.DictReader(text.splitlines(), delimiter="\t" if suffix == ".tsv" else ",")
            return [dict(row) for row in reader], None
        if suffix in (".jsonl", ".ndjson"):
            rows = [json.loads(line) for line in text.splitlines() if line.strip()]
        elif suffix == ".json":
            # The editor's reader: rows, {"rows", "weight"} or columns.
            rows, weight = _core.json_rows(text)
        else:
            raise RulesError(f"{path}: data is read from .csv, .tsv, .json, .jsonl or .ndjson")
    except (json.JSONDecodeError, TypeError, ValueError) as e:
        raise RulesError(f"{path}: {e}") from None
    if not isinstance(rows, list) or not all(isinstance(r, dict) for r in rows):
        raise RulesError(f"{path}: rows are objects (name: value)")
    return rows, weight


def _version() -> str:
    from . import __version__  # noqa: PLC0415 - the package imports this module

    return __version__


def _sha256_of(path: Union[str, "os.PathLike[str]"]) -> str:
    try:
        return hashlib.sha256(Path(path).read_bytes()).hexdigest()
    except OSError as e:
        raise RulesError(f"{path}: {e.strerror or e}") from None


def fit_main(argv: Optional[Sequence[str]] = None, *, prog: str = "openodd-rules-fit") -> int:
    """``openodd-rules-fit``: fit a rule file to data, by maximum likelihood."""
    parser = argparse.ArgumentParser(
        prog=prog,
        description="Fit a rule file's distributions to measured data by maximum likelihood, "
        "and write the fitted parts as a rule file extending it.",
    )
    parser.add_argument("rules", help="the rule file to fit (a path or an https:// URL)")
    parser.add_argument("data", nargs="+", type=Path,
                        help="data files: .csv/.tsv with a header, .jsonl, or .json")
    parser.add_argument("-o", "--output", type=Path, required=True,
                        help="where to write the fitted rule file")
    parser.add_argument("-q", "--quantity", action="append", dest="quantities", metavar="NAME",
                        help="fit only this quantity (repeatable; default: every one the data gives)")
    parser.add_argument("--weight", metavar="COLUMN", help="a column weighing each row")
    parser.add_argument("--fix", action="append", default=[], metavar="SPEC",
                        help="hold values as written, besides the rule file's own fixed: "
                        "QUANTITY, QUANTITY.PART, QUANTITY.PART.PARAM, "
                        "QUANTITY.PART.PARAM@INPUT=POINT[,...] or QUANTITY.PART@INPUT=POINT "
                        "(repeatable)")
    parser.add_argument("--max-iterations", type=int, default=1000,
                        help="the most iterations of each stage of a fit (default: 1000)")
    parser.add_argument("--min-spread", type=float, default=1e-3,
                        help="the least width of a distribution, as a share of the values' "
                        "standard deviation (default: 0.001)")
    parser.add_argument("--overlay", action="store_true",
                        help="write only the fitted parts, not extending the rules fitted")
    args = parser.parse_args(argv)
    try:
        rules = Rules.read(args.rules)
        rows: list[dict[str, Any]] = []
        # The weight column: --weight, else the one a JSON file names.
        weight = args.weight
        for path in args.data:
            part, named = read_data(path)
            rows.extend(part)
            weight = weight or named
        fit = rules.fit(
            rows,
            weight_column=weight,
            quantities=args.quantities,
            max_iterations=args.max_iterations,
            min_spread=args.min_spread,
            fixed=args.fix,
            data_files=args.data,
            by=f"{prog} {_version()}",
        )
        fit.write(args.output, None if args.overlay else args.rules)
    except RulesError as e:
        print(f"{prog}: {e}", file=sys.stderr)
        return 2
    print(f"{len(rows)} rows")
    if not fit.quantities:
        print("no quantity was fitted: no row gives one with every value it reads")
    for q in fit.quantities.values():
        state = "converged" if q.converged else "stopped at --max-iterations"
        print(
            f"{q.name}: {q.rows} rows ({q.skipped} skipped), log-likelihood "
            f"{q.log_likelihood_before:.6g} -> {q.log_likelihood:.6g}, "
            f"{q.iterations} iterations, {state}"
        )
        b, a = q.calibration_before, q.calibration
        print(
            f"  in the central 90%: {b.in_band:.1%} -> {a.in_band:.1%} of rows "
            f"(KS {b.ks:.3f} -> {a.ks:.3f}; 90% and 0 when the rules match)"
        )
        for p in q.parts:
            print(
                f"  {p.name}: {p.points_with_data} of {p.points} points with data, "
                f"{p.estimated} values estimated"
            )
    print(f"wrote {args.output}")
    return 0


def _values(inputs: Optional[Mapping[str, float]]) -> dict[str, float]:
    if not inputs:
        return {}
    return {str(k): float(v) for k, v in inputs.items()}
