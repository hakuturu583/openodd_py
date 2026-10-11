"""Sample concrete cases from an ODD (the drawing is the Rust core's).

What can be drawn are the attributes a run *sets*. Each needs a knob -- the
setting that brings it about -- and three things can give one, none knowing
the others:

* ``knobs``, keyed by the ODD's attributes, for one sampler alone;
* the application's backend (:meth:`Backend.knob`): the knobs it always has
  (a simulator's weather), and those a scenario's ``controls`` give for what
  the backend reads (``measure:`` keys);
* nothing: the attribute is left open, and a combination is admitted when the
  ODD can hold for some value of it.

``strategy="coverage"`` draws the least covered buckets first and aims the
first cases at the situations still uncovered. A batch of samplers on one ODD
goes on where the last left off (:class:`OddDraws`).
"""

from __future__ import annotations

import logging
import math
from collections.abc import Iterable, Mapping, Sequence
from dataclasses import dataclass, field
from typing import Any, Optional, Union

from . import _core
from .backend import Backend, default_backend
from .items import value_label
from .model import OddDefinition, to_core
from .report import CoverageReport

__all__ = ["OddDraws", "OddKnob", "OddSample", "OddSampler", "knobs_from_mapping"]

logger = logging.getLogger(__name__)

KnobValue = Union[float, int, str, bool, Sequence[float]]


def _is_range(value: Any) -> bool:
    return isinstance(value, (list, tuple))


def _range(value: Any) -> tuple[float, float]:
    if len(value) != 2:
        raise ValueError(f"a range is [low, high], not {list(value)!r}")
    return float(value[0]), float(value[1])


def _limits(value: Any) -> tuple[float, float]:
    if len(value) != 2:
        raise ValueError(f"limits are [low, high], not {list(value)!r}")
    low = -math.inf if value[0] is None else float(value[0])
    high = math.inf if value[1] is None else float(value[1])
    return low, high


@dataclass(frozen=True)
class OddKnob:
    """How a run sets an ODD attribute: the setting that does it.

    Attributes:
        key: The setting the value is written to, e.g. ``environment.precipitation``.
        values: Per bucket label: for a categorical attribute, the value to
            write, or ``[low, high)`` to draw it from; for a numeric one, a
            range to draw from instead of the bucket's interval.
        scale, offset: A numeric value, drawn in the attribute's unit, is
            written as ``value * scale + offset``.
        integer: Write an integer.
        range: For a numeric attribute, ``[low, high]`` in its unit: what the
            run can bring about.
        limits: ``[low, high]`` the written value must lie in (``None``: open).
    """

    key: str
    values: Mapping[str, KnobValue] = field(default_factory=dict)
    scale: float = 1.0
    offset: float = 0.0
    integer: bool = False
    range: Optional[tuple[float, float]] = None
    limits: Optional[tuple[float, float]] = None

    def __post_init__(self) -> None:
        if self.range is not None:
            object.__setattr__(self, "range", _range(self.range))
        if self.limits is not None:
            object.__setattr__(self, "limits", _limits(self.limits))
        self._core()  # validates

    def _core(self) -> Any:
        values = {
            str(label): (_range(v) if _is_range(v) else to_core(v)) for label, v in self.values.items()
        }
        return _core.Knob(
            self.key,
            values,
            scale=float(self.scale),
            offset=float(self.offset),
            integer=bool(self.integer),
            range=self.range,
            limits=self.limits,
        )

    @property
    def reach(self) -> Optional[tuple[float, float]]:
        """``range`` within what ``limits`` lets be written, in the attribute's unit."""
        return self._core().reach


def knobs_from_mapping(raw: Optional[Mapping[str, Any]]) -> dict[str, OddKnob]:
    """Knobs by attribute name, from config (``{attribute: {key: ..., ...}}``).

    A bare string is a key. The fields are :class:`OddKnob`'s.
    """
    knobs: dict[str, OddKnob] = {}
    for name, spec in (raw or {}).items():
        if isinstance(spec, str):
            knobs[str(name)] = OddKnob(spec)
            continue
        if not isinstance(spec, Mapping):
            raise ValueError(f"knob {name}: expected a key or a mapping, got {spec!r}")
        unknown = set(spec) - {"key", "values", "scale", "offset", "integer", "range", "limits"}
        if unknown:
            raise ValueError(f"knob {name}: unknown fields {sorted(unknown)}")
        if "key" not in spec:
            raise ValueError(f"knob {name}: key is required")
        knobs[str(name)] = OddKnob(
            key=str(spec["key"]),
            values={value_label(k): v for k, v in (spec.get("values") or {}).items()},
            scale=float(spec.get("scale", 1.0)),
            offset=float(spec.get("offset", 0.0)),
            integer=bool(spec.get("integer", False)),
            range=None if spec.get("range") is None else _range(spec["range"]),
            limits=None if spec.get("limits") is None else _limits(spec["limits"]),
        )
    return knobs


@dataclass(frozen=True)
class OddSample:
    """One concrete case the sampler drew.

    Attributes:
        index: 0-based position in the batch.
        overrides: The settings that bring it about, ``key=value`` each.
        buckets: Attribute name -> the bucket label drawn for it.
        values: Attribute name -> the value drawn, in the attribute's terms.
        situation: The situation the case was aimed at, if any.
    """

    index: int
    overrides: list[str]
    buckets: dict[str, str]
    values: dict[str, Any]
    situation: Optional[str] = None

    @classmethod
    def _from_core(cls, core: Any) -> "OddSample":
        return cls(core.index, list(core.overrides), dict(core.buckets), dict(core.values), core.situation)

    def _core(self) -> Any:
        return _core.Sample(
            self.index, list(self.overrides), list(self.buckets.items()),
            {k: to_core(v) for k, v in self.values.items()}, self.situation,
        )


class OddDraws:
    """The cases drawn so far in a batch, per ODD.

    Two ODDs are one when they are built alike (:attr:`OddDefinition.structure_digest`),
    so a scenario on another ODD starts afresh.
    """

    def __init__(self) -> None:
        self._drawn: dict[str, list[OddSample]] = {}

    def of(self, odd: OddDefinition) -> list[OddSample]:
        """The cases drawn from *odd* so far, in the order they were drawn."""
        return list(self._drawn.get(odd.structure_digest, ()))

    def add(self, odd: OddDefinition, samples: Iterable[OddSample]) -> None:
        """Record *samples*, drawn from *odd*."""
        self._drawn.setdefault(odd.structure_digest, []).extend(samples)


class OddSampler:
    """Draws concrete cases inside an ODD.

    Args:
        odd: The ODD to draw from.
        knobs: Attribute name -> how a run sets it, for this sampler alone.
            Replaces what the backend gives.
        controls: What the run's scenario can set, keyed as the backend reads
            it (a scenario's ``controls``, by measure); the backend turns them
            into knobs for the attributes it reads that way.
        seed: The same seed, ODD, knobs and coverage give the same cases.
        strategy: ``"uniform"`` or ``"coverage"``.
        coverage: The coverage of earlier runs (:func:`~.report.load_and_merge`),
            read by the ``coverage`` strategy.
        max_tries: Combinations drawn per case before giving up on it.
        drawn: The cases earlier samplers of the batch drew from this ODD
            (:meth:`OddDraws.of`).
        backend: Whose knobs (the default backend if none).
    """

    def __init__(
        self,
        odd: OddDefinition,
        knobs: Optional[Mapping[str, OddKnob]] = None,
        *,
        controls: Optional[Mapping[str, OddKnob]] = None,
        seed: Union[int, str] = 0,
        strategy: str = "uniform",
        coverage: Optional[CoverageReport] = None,
        max_tries: int = 200,
        drawn: Sequence[OddSample] = (),
        backend: Optional[Backend] = None,
    ) -> None:
        backend = backend or default_backend()
        knobs = dict(knobs or {})
        controls = dict(controls or {})
        chosen: list[tuple[str, Any]] = []
        names = {a.name for a in odd.attributes}
        unknown = sorted(set(knobs) - names)
        if unknown:
            raise ValueError(f"OddSampler: ODD {odd.name} has no attributes {unknown}")
        for attribute in odd.attributes:
            knob = knobs.get(attribute.name) or backend.knob(attribute, controls)
            if knob is not None:
                chosen.append((attribute.name, knob._core()))
        self.odd = odd
        self.strategy = strategy
        self._core = _core.Sampler(
            odd._core,
            chosen,
            seed=str(seed),
            strategy=strategy,
            coverage=None if coverage is None else coverage._core,
            max_tries=max_tries,
            drawn=[s._core() for s in drawn],
        )

    @property
    def attributes(self) -> list[str]:
        """The attributes drawn, in the ODD's order."""
        return self._core.attributes

    @property
    def situation_holes(self) -> list[str]:
        """The situations still uncovered, which the first cases are aimed at."""
        return self._core.situation_holes

    def draw(self, index: int = 0, situation: Optional[str] = None) -> OddSample:
        """Draw one case; *situation*, if given, is a module the case aims at."""
        return OddSample._from_core(self._core.draw(index, situation))

    def sample(self, count: int) -> list[OddSample]:
        """Draw *count* cases; with the coverage strategy, the first aim at the
        situations still uncovered, one each."""
        if count < 0:
            raise ValueError("OddSampler: count must not be negative")
        cases = [OddSample._from_core(c) for c in self._core.sample(count)]
        for warning in self._core.take_warnings():
            logger.warning(warning)
        return cases
