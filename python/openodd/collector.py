"""Sample cover items during a run and write what they hit.

A backend drives :class:`CoverageCollector`: :meth:`~CoverageCollector.start`
when the run's clock starts, :meth:`~CoverageCollector.tick` after every step
(a simulator tick, a log row), :meth:`~CoverageCollector.end` when the run is
over. Each takes the backend's *context* -- what a step is measured on -- and
the elapsed seconds. The collector is best effort: an expression that raises
is logged once and counted as no sample, and nothing here can fail a run. An
expression that returns ``None`` has nothing to say and is not a sample either.

With an ODD, every step also samples the ODD's attributes, once each
(:meth:`OddDefinition.sample`). Those values fill the ODD's cover items and
decide whether the step was inside the ODD, and the step (its seconds and the
ego's metres) is counted in its *cell* -- one bucket of each attribute
(:class:`~.cells.OddCells`) -- so the run can be sliced by any attributes
later. An ODD read from files is written beside the coverage file as its
transmission archive (``<name>_odd.tar.gz``).

Every coverage file says what wrote it (``generator``: this library's
version, and whatever the backend adds), so files from different versions
can be told apart.

What only the backend knows it asks the :class:`~.backend.Backend`: where the
ego is (:meth:`~.backend.Backend.position`, in metres, for the distance a
step stands for) and where on Earth (:meth:`~.backend.Backend.geolocation`,
for the ODD samples a COD is exported from, :mod:`.cod`). An item sampled on
an event of the backend's own is sampled each time ``event.check(context,
elapsed)`` turns from ``None`` to something else.

The coverage file (``{scenario}_coverage.json``, schema
:data:`~.report.COVERAGE_SCHEMA`) holds, per bucket of one run, its hits and,
for items sampled every step, its exposure: the seconds spent in it, the
metres the ego drove in it, and how many times it was entered.

A run drawn by a sampler can also record its case
(:meth:`CoverageCollector.record_case`: its values, buckets and settings, why
it was chosen, the flakiness predicted when a flaky search chose it, and the
space it was chosen in) and its outcome
(:meth:`CoverageCollector.record_outcome`). The merged report then assesses
the flakiness of every such run over all of them, and the next campaign's
flaky search can start from them (:meth:`~.flaky.FlakySearch.observe_report`).
An outcome can also say when the run failed and what it ran on (its
environment): the report then counts, per ODD cell, the runs that drove
through it, how many failed, and how many failed in it
(:attr:`~.report.CoverageReport.cell_flakiness`).
:func:`~.report.merge_coverage` merges any number of them into a report.
"""

from __future__ import annotations

import dataclasses
import itertools
import json
import logging
import math
from datetime import datetime, timezone
from pathlib import Path
from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, Optional

from .backend import Backend, default_backend
from .cells import OddCells

from .items import (
    ABOVE_RANGE,
    BELOW_RANGE,
    COVER_MEASURES,
    CROSS_SEPARATOR,
    HOLDS,
    CoverItem,
    CrossItem,
    Event,
    SamplingEvent,
    duplicates,
    value_label,
)
from .report import COVERAGE_SCHEMA

if TYPE_CHECKING:
    from .model import OddDefinition, OddVerdict

__all__ = [
    "COVERAGE_SCHEMA",
    "CROSS_SEPARATOR",
    "ODD_ARCHIVE_SUFFIX",
    "CoverageCollector",
    "generator",
]

logger = logging.getLogger(__name__)

#: Beside a run's coverage file, the transmission archive of its ODD.
ODD_ARCHIVE_SUFFIX = "_odd.tar.gz"


def generator(extra: Optional[Mapping[str, Optional[str]]] = None) -> dict[str, Optional[str]]:
    """What writes a coverage file: this library's version, then *extra*."""
    from . import __version__  # noqa: PLC0415 - the package imports this module

    return {"openodd": __version__, **dict(extra or {})}



#: The measures a bucket keeps, in the order a stay is recorded.
_MEASURES = COVER_MEASURES

#: Faster than this between two ticks, the ego was moved, not driven (a
#: respawn, a teleport): the step adds no distance.
MAX_EGO_SPEED_MPS = 100.0

#: Out-of-ODD intervals kept per run; a run that leaves the ODD more often is
#: still counted in full, only the intervals past this are not listed.
MAX_OUT_INTERVALS = 100


def _sample_value(value: Any) -> Any:
    """A value as the coverage file keeps it: a JSON number, bool or string."""
    if hasattr(value, "item") and not isinstance(value, (str, bytes)):
        try:
            value = value.item()  # a numpy scalar: its Python value
        except (TypeError, ValueError):
            pass
    if value is None or isinstance(value, (bool, int)):
        return value
    if isinstance(value, str):
        return value  # a literal, however numeric it looks ("001", "nan")
    if isinstance(value, float):
        return round(value, 6) if math.isfinite(value) else None
    try:
        number = float(value)  # numpy scalars and the like
    except (TypeError, ValueError):
        return value_label(value)
    return round(number, 6) if math.isfinite(number) else None


def _case_value(value: Any) -> Any:
    """A case's value, exactly: a JSON number, bool or its label."""
    if hasattr(value, "item") and not isinstance(value, (str, bytes)):
        try:
            value = value.item()  # a numpy scalar: its Python value
        except (TypeError, ValueError):
            pass
    if isinstance(value, (bool, int, str)):
        return value
    if isinstance(value, float):
        return value if math.isfinite(value) else None
    return value_label(value)


def _is_nan(value: Any) -> bool:
    """Whether *value* is a NaN: a float's, or a NumPy scalar's (float32 too)."""
    if isinstance(value, (str, bytes, bool)):
        return False
    try:
        return math.isnan(value)
    except (TypeError, ValueError):
        return False


def _attribute_meta(attribute: Any) -> dict[str, Any]:
    """What an exporter needs to know of an attribute: its name, unit and values."""
    item = attribute.item
    values = list(item.labels) if item is not None and not item.numeric else None
    return {"name": attribute.name, "unit": attribute.unit, "values": values}


def _situation_value(holds: Any) -> Optional[str]:
    """A situation's sample: it held, it rested on missing values, or nothing."""
    if holds is True:
        return HOLDS
    if holds is None:
        return UNKNOWN_SITUATION  # counted apart, and the stay is over
    return None  # failed, or inactive


#: A situation whose verdict rested on missing values.
UNKNOWN_SITUATION = "unknown"


class _OddMonitor:
    """Whether each tick was inside the ODD, and which modules ruled it out.

    A tick is ``inside``, ``outside``, or ``assumed``: inside only because
    values were missing, which OpenODD's missing-value semantics count as
    inside.
    """

    def __init__(self, odd: "OddDefinition") -> None:
        self.odd = odd
        #: When the run started (UTC, ISO 8601), set by the collector.
        self.started_at: Optional[str] = None
        #: One row per tick: elapsed seconds, latitude, longitude, then every
        #: attribute's value (``None`` when missing).  ``odd.samples`` in the
        #: coverage file; ``scenario-coverage --export-cod`` makes a COD of it.
        self.rows: list[list[Any]] = []
        self.ticks = {"inside": 0, "assumed": 0, "outside": 0}
        self.seconds = {"inside": 0.0, "assumed": 0.0, "outside": 0.0}
        # Bounds only: a situation's ticks are its own cover item.
        self.modules = {
            m.name: {"failed_ticks": 0, "missing_ticks": 0}
            for m in odd.modules
            if not m.situation
        }
        self.out_intervals: list[list[float]] = []
        #: Whether the excursion going on is the last interval listed.
        self._recording = False
        #: Steps, seconds and metres per combination of the attributes'
        #: buckets (``odd.cells``).
        self.cells = OddCells(odd)
        self._cell_failed = False
        #: Each tick's time (as given, unrounded) and cell (an index into
        #: ``_cell_labels``; ``None`` when it could not be counted), in order.
        self._tick_cells: list[tuple[float, Optional[int]]] = []
        self._cell_index: dict[tuple[str, ...], int] = {}
        self._cell_labels: list[list[str]] = []

    def count(
        self, values: dict[str, Any], seconds: float, meters: float, elapsed: float
    ) -> None:
        """Count one step, which ended at *elapsed*, in its cell (best effort,
        logged once)."""
        try:
            cell = self.cells.add(
                [values.get(a.name) for a in self.odd.attributes], seconds, meters
            )
            key = tuple(cell)
            if key not in self._cell_index:
                self._cell_index[key] = len(self._cell_labels)
                self._cell_labels.append(cell)
            self._tick_cells.append((elapsed, self._cell_index[key]))
        except Exception:
            self._tick_cells.append((elapsed, None))
            if not self._cell_failed:
                self._cell_failed = True
                logger.warning(
                    "coverage: could not count a step in the ODD's cells (logged once)",
                    exc_info=True,
                )

    def record(
        self,
        values: dict[str, Any],
        start: float,
        end: float,
        position: Optional[tuple[float, float]] = None,
    ) -> "OddVerdict":
        """Judge one tick and keep its values; the verdict, for the situations."""
        lat, lon = (
            (round(position[0], 7), round(position[1], 7))
            if position is not None
            else (None, None)
        )
        self.rows.append(
            [round(end, 3), lat, lon]
            + [_sample_value(values.get(a.name)) for a in self.odd.attributes]
        )
        verdict = self.odd.evaluate(values)
        if not verdict.inside:
            key = "outside"
        else:
            key = "assumed" if verdict.assumed else "inside"
        self.ticks[key] += 1
        self.seconds[key] += max(0.0, end - start)
        for name, holds in verdict.modules.items():
            if name not in self.modules:
                continue  # a situation
            if holds is False:
                self.modules[name]["failed_ticks"] += 1
            elif holds is None:
                self.modules[name]["missing_ticks"] += 1
        outside = not verdict.inside
        if outside and self._recording:
            self.out_intervals[-1][1] = end
        elif outside and len(self.out_intervals) < MAX_OUT_INTERVALS:
            self.out_intervals.append([start, end])
            self._recording = True
        if not outside:
            self._recording = False
        return verdict

    def cell_at(self, elapsed: float) -> Optional[list[str]]:
        """The cell of the last tick, in the order ticked, that ended at or
        before *elapsed* (the samples' clock; after a clock that went back,
        a later tick wins); ``None`` before the first tick, or if it was not
        counted."""
        for at, index in reversed(self._tick_cells):
            if at <= elapsed:
                return None if index is None else list(self._cell_labels[index])
        return None

    def to_dict(self) -> dict[str, Any]:
        return {
            **self.odd.describe(),
            "ticks": dict(self.ticks),
            "seconds": {k: round(v, 3) for k, v in self.seconds.items()},
            "module_ticks": {k: dict(v) for k, v in self.modules.items()},
            "out_intervals": [
                [round(a, 3), round(b, 3)] for a, b in self.out_intervals
            ],
            "started_at": self.started_at,
            "samples": {
                "attributes": [_attribute_meta(a) for a in self.odd.attributes],
                "rows": self.rows,
            },
            "cells": self.cells.to_dict(),
        }


class _Exposure:
    """Per bucket: hits, and for samples taken every tick, time, distance, entries.

    A sample on a tick stands for the step since the previous tick: its
    seconds and the metres the ego drove.  A stay is a run of consecutive
    ticks in one bucket; entering it again after a tick elsewhere, or after a
    tick with no sample, is a new entry.  A one-shot sample is an entry of no
    duration.
    """

    def __init__(self, labels: list[str], min_stay: Optional[float] = None) -> None:
        self.hits: dict[str, int] = {label: 0 for label in labels}
        self.seconds: dict[str, float] = {label: 0.0 for label in labels}
        self.meters: dict[str, float] = {label: 0.0 for label in labels}
        self.entries: dict[str, int] = {label: 0 for label in labels}
        #: The bucket of the stay going on, if any.
        self._stay: Optional[str] = None
        #: With a minimum stay, the measures of the stays that lasted long
        #: enough, and the stay going on: [bucket, hits, seconds, meters].
        self.min_stay = min_stay
        self._counted: dict[str, dict[str, float]] = {
            m: {label: 0 for label in labels} for m in _MEASURES
        }
        self._pending: Optional[list[Any]] = None

    def add(self, label: str, step: Optional["_Step"]) -> None:
        self.hits[label] += 1
        if step is None:
            self.entries[label] += 1
            return
        if label != self._stay:
            self.entries[label] += 1
            self._settle()
            self._pending = [label, 0, 0.0, 0.0]
        self._stay = label
        self.seconds[label] += step.seconds
        self.meters[label] += step.meters
        if self._pending is not None:
            self._pending[1] += 1
            self._pending[2] += step.seconds
            self._pending[3] += step.meters

    def gap(self) -> None:
        """A tick without a sample in any bucket: the stay is over."""
        self._settle()
        self._stay = None

    def _settle(self) -> None:
        """Count the stay that just ended, if it lasted long enough."""
        pending, self._pending = self._pending, None
        if pending is not None and self._long_enough(pending):
            self._count(self._counted, pending)

    def _long_enough(self, pending: list[Any]) -> bool:
        return self.min_stay is not None and pending[2] >= self.min_stay - 1e-9

    @staticmethod
    def _count(into: dict[str, dict[str, float]], pending: list[Any]) -> None:
        label = pending[0]
        for measure, amount in zip(_MEASURES, (pending[1], pending[2], pending[3], 1)):
            into[measure][label] += amount

    def counted(self) -> dict[str, dict[str, float]]:
        """The measures of the stays at least ``min_stay`` long, the last included."""
        out = {m: dict(v) for m, v in self._counted.items()}
        if self._pending is not None and self._long_enough(self._pending):
            self._count(out, self._pending)
        return out

    def to_dict(self) -> dict[str, Any]:
        out: dict[str, Any] = {
            "hits": dict(self.hits),
            "seconds": {k: round(v, 3) for k, v in self.seconds.items()},
            "meters": {k: round(v, 3) for k, v in self.meters.items()},
            "entries": dict(self.entries),
        }
        if self.min_stay is not None:
            out["counted"] = {
                m: {k: round(v, 3) if isinstance(v, float) else v for k, v in d.items()}
                for m, d in self.counted().items()
            }
        return out


class _Step:
    """The step one tick stands for: its duration, and the ego's distance."""

    __slots__ = ("seconds", "meters")

    def __init__(self, seconds: float, meters: float) -> None:
        self.seconds = seconds
        self.meters = meters


class _ItemHits:
    def __init__(self, item: CoverItem, outside: list[str]) -> None:
        self.item = item
        #: Buckets outside the run's ODD: reported, but not coverage targets.
        self.outside = outside
        self.exposure = _Exposure(item.labels, item.min_stay)
        self.hits = self.exposure.hits
        self.out_of_range: dict[str, int] = {}
        self.ignored = 0
        self.samples = 0
        self.failed = False

    def to_dict(self) -> dict[str, Any]:
        return {
            **self.item.describe(),
            "outside_odd": list(self.outside),
            "samples": self.samples,
            "ignored": self.ignored,
            **self.exposure.to_dict(),
            "out_of_range": dict(self.out_of_range),
        }


class _CrossHits:
    def __init__(self, cross: CrossItem, outside: dict[str, list[str]]) -> None:
        self.cross = cross
        cells = list(itertools.product(*(i.labels for i in cross.items)))
        self.exposure = _Exposure(
            [CROSS_SEPARATOR.join(c) for c in cells], cross.min_stay
        )
        self.hits = self.exposure.hits
        self.outside = [
            CROSS_SEPARATOR.join(cell)
            for cell in cells
            if any(
                label in outside.get(item.name, ())
                for item, label in zip(cross.items, cell)
            )
        ]

    def to_dict(self) -> dict[str, Any]:
        return {
            **self.cross.describe(),
            "buckets": list(self.hits),
            "outside_odd": list(self.outside),
            **self.exposure.to_dict(),
        }


class CoverageCollector:
    """Samples a run's cover items on their events and counts bucket hits."""

    def __init__(
        self,
        items: list[CoverItem],
        crosses: Optional[list[CrossItem]] = None,
        odd: Optional["OddDefinition"] = None,
        *,
        backend: Optional[Backend] = None,
        generator: Optional[Mapping[str, Optional[str]]] = None,
    ) -> None:
        """Collect *items* and *crosses*, and the attributes of *odd* if given.

        With an *odd*, its attributes' cover items come first, and every step
        is also judged inside or outside it. *backend* says where the ego is
        (the default backend if none). *generator* names, besides this
        library, what wrote the file (an application and its version).
        """
        self._backend = backend or default_backend()
        self._generator = dict(generator or {})
        self._odd = _OddMonitor(odd) if odd is not None else None
        # ODD item -> the attribute whose sampled value fills it.
        self._odd_items: dict[str, str] = {}
        # Situation item -> the module whose verdict fills it.
        self._situation_items: dict[str, str] = {}
        outside: dict[str, list[str]] = {}
        if odd is not None:
            for attribute in odd.attributes:
                if attribute.item is not None:
                    self._odd_items[attribute.item.name] = attribute.name
                    outside[attribute.item.name] = odd.outside_buckets(attribute)
            situations = odd.situations()
            self._situation_items = {
                m.item.name: m.name for m in situations if m.item is not None
            }
            items = [
                *odd.cover_items(),
                *(m.item for m in situations if m.item is not None),
                *items,
            ]
        crosses = list(crosses or ())
        names = [i.name for i in items] + [c.name for c in crosses]
        if duplicates(names):
            raise ValueError(
                f"coverage: names used more than once: {duplicates(names)}"
            )
        self._items = [_ItemHits(i, outside.get(i.name, [])) for i in items]
        self._crosses = [_CrossHits(c, outside) for c in crosses or ()]
        # Items and crosses by the event they are sampled on.
        self._items_on: dict[int, list[_ItemHits]] = {}
        for entry in self._items:
            self._items_on.setdefault(id(entry.item.event), []).append(entry)
        self._crosses_on: dict[int, list[_CrossHits]] = {}
        for cross in self._crosses:
            self._crosses_on.setdefault(id(cross.cross.event), []).append(cross)
        # Conditions an item is sampled on, each with whether it was satisfied
        # on the previous tick: an item is sampled when one becomes satisfied,
        # not on every tick it stays so.
        self._conditions: dict[int, list[Any]] = {
            id(e.item.event): [e.item.event, False]
            for e in self._items
            if not isinstance(e.item.event, SamplingEvent)
        }
        self._failed_conditions: set[int] = set()
        self._last_elapsed = 0.0
        self._last_position: Optional[tuple[float, float, float]] = None
        # Whether anything is sampled every tick, and so measures distance.
        tick = id(SamplingEvent.TICK)
        self._timed = tick in self._items_on or tick in self._crosses_on
        self._case: Optional[dict[str, Any]] = None
        self._passed: Optional[bool] = None
        self._failed_at: Optional[float] = None
        self._environment: Optional[str] = None

    def record_case(self, case: Any, space: Optional[Any] = None) -> None:
        """Record the case this run ran: an :class:`~.sampler.OddSample`.

        Its flakiness, when a flaky search chose it for that
        (``case.flaky``), is the prediction made then. *space* is the
        :class:`~.flaky.ParameterSpace` it was drawn in (``search.space``):
        without it, the report cannot assess the run's flakiness.
        """
        flaky = getattr(case, "flaky", None)
        self._case = {
            "index": int(case.index),
            "situation": case.situation,
            "values": {str(k): _case_value(v) for k, v in case.values.items()},
            "buckets": {str(k): str(v) for k, v in case.buckets.items()},
            "overrides": list(case.overrides),
            "chosen_for": "coverage" if flaky is None else "instability",
            "flaky": None if flaky is None else dataclasses.asdict(flaky),
            "space": None if space is None else space.to_list(),
        }

    def record_outcome(
        self,
        passed: bool,
        *,
        failed_at: Optional[float] = None,
        environment: Optional[str] = None,
    ) -> None:
        """Record whether the run passed.

        Args:
            passed: Whether it passed.
            failed_at: When a failed run failed, in seconds on the clock the
                collector is given (``elapsed`` of :meth:`start` and
                :meth:`tick`, the samples' time column): seconds since
                ``started_at`` when that clock starts at zero. With an ODD,
                the cell of the last tick at or before it is written too
                (``outcome.failed_cell``). Ignored, with a warning, for a run
                that passed, or if not a finite number (a bool or a string is
                not one).
            environment: What the run ran on, as an opaque ID (a digest of
                the software and its dependencies' versions): the report
                pools per-cell outcomes only over runs of the same one.

        Each call replaces what an earlier one recorded, the time and the
        environment included.
        """
        self._passed = bool(passed)
        self._failed_at = None
        if failed_at is not None:
            at = math.nan
            if isinstance(failed_at, (int, float)) and not isinstance(failed_at, bool):
                at = float(failed_at)
            if self._passed:
                logger.warning("coverage: failed_at=%r of a run that passed is ignored", failed_at)
            elif not math.isfinite(at):
                logger.warning("coverage: failed_at=%r is not a time; ignored", failed_at)
            else:
                self._failed_at = at  # compared unrounded; written as the samples' times are
        self._environment = None if environment is None else str(environment)

    def _outcome(self) -> Optional[dict[str, Any]]:
        """The coverage file's ``outcome``: only what was recorded."""
        if self._passed is None:
            return None
        outcome: dict[str, Any] = {"passed": self._passed}
        if self._failed_at is not None:
            outcome["failed_at"] = round(self._failed_at, 3)
            cell = self._odd.cell_at(self._failed_at) if self._odd is not None else None
            if cell is not None:
                outcome["failed_cell"] = cell
        if self._environment is not None:
            outcome["environment"] = self._environment
        return outcome

    def start(self, context: Any, elapsed: float) -> None:
        """Sample the items on :attr:`SamplingEvent.START`."""
        self._last_elapsed = elapsed
        self._last_position = self._ego_position(context) if self._timed else None
        if self._odd is not None:
            self._odd.started_at = datetime.now(timezone.utc).isoformat(
                timespec="milliseconds"
            )
        self._sample_event(context, SamplingEvent.START)

    def tick(self, context: Any, elapsed: float) -> None:
        """Sample the items on :attr:`SamplingEvent.TICK` and on conditions that fired."""
        values: dict[str, Any] = {}
        if self._odd is not None:
            attributes = self._odd.odd.sample(context)
            values = {item: attributes[a] for item, a in self._odd_items.items()}
            verdict = self._odd.record(
                attributes, self._last_elapsed, elapsed, self._geolocation(context)
            )
            for item, module in self._situation_items.items():
                values[item] = _situation_value(verdict.modules.get(module))
        step = self._step(context, elapsed)
        if self._odd is not None:
            self._odd.count(attributes, step.seconds, step.meters, elapsed)
        self._sample_event(context, SamplingEvent.TICK, values, step)
        for key, entry in self._conditions.items():
            condition, was_satisfied = entry
            try:
                satisfied = condition.check(context, elapsed) is not None
            except Exception:
                if key not in self._failed_conditions:
                    self._failed_conditions.add(key)
                    logger.warning(
                        "coverage: sampling condition %r raised; it stays "
                        "unsatisfied (logged once)",
                        getattr(condition, "label", condition),
                        exc_info=True,
                    )
                satisfied = False
            entry[1] = satisfied
            if satisfied and not was_satisfied:
                self._sample_event(context, condition)

    def end(self, context: Any, elapsed: float) -> None:
        """Sample the items on :attr:`SamplingEvent.END`."""
        self._sample_event(context, SamplingEvent.END)

    def to_dict(self, scenario: str) -> dict[str, Any]:
        """One run's coverage, in the coverage-file format."""
        return {
            "schema": COVERAGE_SCHEMA,
            "generator": generator(self._generator),
            "scenario": scenario,
            "items": [e.to_dict() for e in self._items],
            "crosses": [c.to_dict() for c in self._crosses],
            "odd": self._odd.to_dict() if self._odd is not None else None,
            "case": self._case,
            "outcome": self._outcome(),
        }

    def write(self, path: Path, scenario: str) -> None:
        """Write the coverage file; a failure is logged, never raised.

        An ODD read from files is written beside it too, as its transmission
        archive (``<name>_odd.tar.gz``, :meth:`OddDefinition.archive`): the
        exact files the run was measured against, by their revision.
        """
        try:
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(
                json.dumps(self.to_dict(scenario), indent=2), encoding="utf-8"
            )
        except Exception:
            logger.warning("coverage: could not write %s", path, exc_info=True)
        if self._odd is None:
            return
        try:
            archive = self._odd.odd.archive()
            if archive is not None:
                stem = path.name.removesuffix(".json").removesuffix("_coverage")
                (path.parent / f"{stem}{ODD_ARCHIVE_SUFFIX}").write_bytes(archive)
        except Exception:
            logger.warning("coverage: could not write the ODD's archive", exc_info=True)

    # ------------------------------------------------------------------

    def _step(self, context: Any, elapsed: float) -> _Step:
        """The step since the previous tick; remembers this tick's time and place."""
        seconds = max(0.0, elapsed - self._last_elapsed)
        # A clock that went back does not move the start of the next step.
        self._last_elapsed = max(elapsed, self._last_elapsed)
        if not self._timed:
            return _Step(seconds, 0.0)  # nothing measures distance: no lookup
        position = self._ego_position(context)
        meters = 0.0
        if position is not None and self._last_position is not None:
            meters = math.dist(position, self._last_position)
            # NaN (a physics blow-up) or a jump (a respawn) is not driving.
            if not math.isfinite(meters) or meters > MAX_EGO_SPEED_MPS * max(
                seconds, 0.05
            ):
                meters = 0.0
        self._last_position = position
        return _Step(seconds, meters)

    def _geolocation(self, context: Any) -> Optional[tuple[float, float]]:
        try:
            return self._backend.geolocation(context)
        except Exception:
            return None

    def _ego_position(self, context: Any) -> Optional[tuple[float, float, float]]:
        try:
            return self._backend.position(context)
        except Exception:
            return None

    def _sample_event(
        self,
        context: Any,
        event: Event,
        values: Optional[dict[str, Any]] = None,
        step: Optional[_Step] = None,
    ) -> None:
        """Sample the items on *event*; *values* holds those sampled already.

        *step* is the step a tick stands for; ``None`` for a one-shot event.
        """
        entries = self._items_on.get(id(event), ())
        if not entries:
            return
        buckets: dict[str, Optional[str]] = {}
        for entry in entries:
            name = entry.item.name
            if values is not None and name in values:
                value = values[name]
            else:
                value = self._evaluate(entry, context)
            label = self._count(entry, value)
            buckets[name] = label
            if label is None:
                entry.exposure.gap()
            else:
                entry.exposure.add(label, step)
        for cross in self._crosses_on.get(id(event), ()):
            cell = [buckets.get(i.name) for i in cross.cross.items]
            key = CROSS_SEPARATOR.join(cell) if None not in cell else None  # type: ignore[arg-type]
            if key is not None and key in cross.hits:
                cross.exposure.add(key, step)
            else:
                cross.exposure.gap()

    def _evaluate(self, entry: _ItemHits, context: Any) -> Any:
        try:
            return entry.item.expression(context)
        except Exception:
            self._fail_once(entry, "expression raised")
            return None

    def _count(self, entry: _ItemHits, value: Any) -> Optional[str]:
        """Count *value* for one item; the bucket it hit, or ``None`` when it hit none."""
        if value is None or _is_nan(value):
            return None  # NaN (say, 0/0) says nothing either
        item = entry.item
        try:
            if item.ignore is not None and item.ignore(value):
                entry.ignored += 1
                return None
            label = item.bucket_of(value)
        except Exception:
            self._fail_once(entry, f"cannot bucket {value!r}")
            return None
        entry.samples += 1
        if label in entry.hits:
            return label
        if label in (BELOW_RANGE, ABOVE_RANGE) or not item.numeric:
            entry.out_of_range[label] = entry.out_of_range.get(label, 0) + 1
        return None

    @staticmethod
    def _fail_once(entry: _ItemHits, what: str) -> None:
        if entry.failed:
            logger.debug("coverage: %s: %s", entry.item.name, what, exc_info=True)
            return
        entry.failed = True
        logger.warning(
            "coverage: %s: %s; counted as no sample (logged once)",
            entry.item.name,
            what,
            exc_info=True,
        )
