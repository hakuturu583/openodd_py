"""Cover items: what is sampled, when, and how a value is put in a bucket.

The model follows ``cover()`` in ASAM OpenSCENARIO DSL (section 7.5): ``range``
+ ``every``, ``buckets`` (edges) or ``values`` make the buckets; ``target`` is
what covering one takes. Beyond the DSL, an item sampled every step can count
its target in seconds, metres or entries (``cover_by``) and leave out stays
shorter than ``min_stay``. The buckets and the labels are the Rust core's.

What samples an item -- a simulator's tick loop, a log reader -- is the
backend's: an item says what to read (``expression``, a callable on the
backend's context) and when (``event``: a :class:`SamplingEvent`, or any object
of the backend's own, named in reports by its ``label``).
"""

from __future__ import annotations

import enum
import math
from collections import Counter
from dataclasses import dataclass, field
from typing import Any, Callable, Iterable, Optional, Sequence, Union

from . import _core

__all__ = [
    "ABOVE_RANGE",
    "BELOW_RANGE",
    "COVER_MEASURES",
    "CROSS_SEPARATOR",
    "HOLDS",
    "CoverGroup",
    "CoverItem",
    "CrossItem",
    "SamplingEvent",
    "duplicates",
    "event_name",
    "value_label",
]

#: Pseudo-bucket a numeric sample below the first bucket is counted in.
BELOW_RANGE = "<below>"
#: Pseudo-bucket a numeric sample above the last bucket is counted in.
ABOVE_RANGE = "<above>"
#: The one bucket of a situation's cover item: the ODD module held.
HOLDS = "holds"
#: Joins the bucket labels of a cross-coverage cell (``"1 / x"``).
CROSS_SEPARATOR = " / "

#: What an item's ``target`` counts.
COVER_MEASURES = ("hits", "seconds", "meters", "entries")


class SamplingEvent(enum.Enum):
    """When a cover item is sampled. ``END`` is the default, as in OpenSCENARIO DSL."""

    #: Once, when the scenario clock starts.
    START = "start"
    #: Once, when the run ends.
    END = "end"
    #: After every step (a simulator tick, a log row): hits are steps.
    TICK = "tick"


class CoverGroup(enum.Enum):
    """Which report a cover item belongs to."""

    ODD = "odd"
    #: ODD modules covered as situations: combinations the runs should drive.
    SITUATION = "situation"
    SCENARIO = "scenario"


#: A built-in event, or a backend's own.
Event = Union[SamplingEvent, Any]


def duplicates(names: Iterable[str]) -> list[str]:
    """The names that occur more than once, sorted."""
    return sorted(n for n, count in Counter(names).items() if count > 1)


def value_label(value: Any) -> str:
    """The bucket label of a categorical value: an enum by name, a bool in lowercase."""
    if isinstance(value, enum.Enum):
        return value.name
    if type(value).__name__ == "bool_":  # numpy
        return "true" if value else "false"
    return _core.value_label(value)


def event_name(event: Event) -> str:
    """How *event* is written in a report."""
    if isinstance(event, SamplingEvent):
        return event.value
    return f"condition:{getattr(event, 'label', type(event).__name__)}"


def _core_value(value: Any) -> Any:
    from .model import to_core  # noqa: PLC0415 - model imports items

    return to_core(value)


@dataclass
class CoverItem:
    """One cover item.

    Exactly one of *values*, *buckets* and *range* (with *every*) defines the
    buckets.
    """

    name: str
    expression: Callable[[Any], Any]
    unit: str = ""
    range: Optional[tuple[float, float]] = None
    every: Optional[float] = None
    buckets: Optional[Sequence[float]] = None
    values: Optional[Iterable[Any]] = None
    ignore: Optional[Callable[[Any], bool]] = None
    event: Event = SamplingEvent.END
    text: str = ""
    target: float = 1
    group: CoverGroup = CoverGroup.SCENARIO
    cover_by: str = "hits"
    min_stay: Optional[float] = None
    #: The ODD attribute the item buckets, when it is one's.
    attribute: Optional[str] = None

    #: Bucket edges of a numeric item, ascending; empty for a categorical one.
    edges: list[float] = field(init=False, default_factory=list)
    #: Bucket labels, in order.
    labels: list[str] = field(init=False, default_factory=list)

    def __post_init__(self) -> None:
        if not self.name:
            raise ValueError("cover: name must not be empty")
        if self.values is not None:
            self.values = list(self.values)
        bucketing = _core.Bucketing(
            self.name,
            values=None if self.values is None else [_core_value(v) for v in self.values],
            buckets=None if self.buckets is None else [float(x) for x in self.buckets],
            range=None if self.range is None else (float(self.range[0]), float(self.range[1])),
            every=None if self.every is None else float(self.every),
            event="tick" if self.event is SamplingEvent.TICK else event_name(self.event),
            target=float(self.target),
            cover_by=self.cover_by,
            min_stay=self.min_stay,
        )
        self._bucketing = bucketing
        self.edges = list(bucketing.edges)
        self.labels = list(bucketing.labels)

    @classmethod
    def _from_bucketing(
        cls,
        bucketing: Any,
        *,
        expression: Callable[[Any], Any],
        values: Optional[list[Any]] = None,
        event: Event = SamplingEvent.TICK,
        group: CoverGroup = CoverGroup.ODD,
    ) -> "CoverItem":
        self = cls.__new__(cls)
        self.name = bucketing.name
        self.expression = expression
        self.unit = bucketing.unit
        self.range = None
        self.every = None
        self.edges = list(bucketing.edges)
        self.labels = list(bucketing.labels)
        self.buckets = self.edges or None
        self.values = None if self.edges else (values if values is not None else list(bucketing.values))
        self.ignore = None
        self.event = event
        self.text = bucketing.text
        target = bucketing.target
        self.target = int(target) if float(target).is_integer() else target
        self.group = group
        self.cover_by = bucketing.cover_by
        self.min_stay = bucketing.min_stay
        self.attribute = bucketing.attribute
        self._bucketing = bucketing
        return self

    @property
    def numeric(self) -> bool:
        """Whether the buckets are ranges rather than values."""
        return bool(self.edges)

    @property
    def amount(self) -> str:
        """The measure a bucket's coverage is read from: ``cover_by``, or with
        a ``min_stay`` the stays long enough to count (``counted.<cover_by>``)."""
        return f"counted.{self.cover_by}" if self.min_stay is not None else self.cover_by

    def bucket_of(self, value: Any) -> str:
        """The label of the bucket *value* falls in.

        :data:`BELOW_RANGE` or :data:`ABOVE_RANGE` for a numeric value outside
        the buckets; a categorical value that is none of the values keeps its
        own label. Each bucket holds its lower edge; the last its upper too.
        """
        if not self.numeric:
            return value_label(value)
        x = float(value)
        if math.isnan(x) or x < self.edges[0]:
            return BELOW_RANGE
        return self._bucketing.bucket_of(x)

    def describe(self) -> dict[str, Any]:
        """The item's definition, as written to a coverage file."""
        return {
            "name": self.name,
            "group": self.group.value,
            "text": self.text,
            "unit": self.unit,
            "event": event_name(self.event),
            "kind": "numeric" if self.numeric else "categorical",
            "buckets": list(self.labels),
            "target": self.target,
            "cover_by": self.cover_by,
            "min_stay": self.min_stay,
            "amount": self.amount,
            **({"attribute": self.attribute} if self.attribute is not None else {}),
        }


@dataclass
class CrossItem:
    """Cross coverage of cover items sampled on the same event."""

    name: str
    items: list[CoverItem]
    text: str = ""
    target: float = 1
    cover_by: str = "hits"
    min_stay: Optional[float] = None

    def __post_init__(self) -> None:
        if not self.name:
            raise ValueError("cross: name must not be empty")
        if len(self.items) < 2:
            raise ValueError(f"cross({self.name}): cross at least two items")
        first = self.items[0]
        for item in self.items[1:]:
            if item.event is not first.event:
                raise ValueError(
                    f"cross({self.name}): {item.name} is sampled on "
                    f"{event_name(item.event)} and {first.name} on "
                    f"{event_name(first.event)}; only items sampled on the "
                    "same event can be crossed"
                )
        if len({i.group for i in self.items}) != 1:
            raise ValueError(f"cross({self.name}): items are in different groups")
        # A cell is its labels joined: one holding the separator would read
        # as another cell.
        for item in self.items:
            joined = [label for label in item.labels if CROSS_SEPARATOR in label]
            if joined:
                raise ValueError(
                    f"cross({self.name}): {item.name}'s buckets {joined} hold "
                    f"{CROSS_SEPARATOR!r}, which joins a cell's labels"
                )
        # The criteria are the core's to check, as for an item.
        _core.Bucketing(
            f"cross({self.name})",
            values=["x"],
            event="tick" if self.event is SamplingEvent.TICK else event_name(self.event),
            target=float(self.target),
            cover_by=self.cover_by,
            min_stay=self.min_stay,
        )

    @property
    def event(self) -> Event:
        return self.items[0].event

    @property
    def group(self) -> CoverGroup:
        return self.items[0].group

    def describe(self) -> dict[str, Any]:
        """The cross's definition, as written to a coverage file."""
        return {
            "name": self.name,
            "group": self.group.value,
            "text": self.text,
            "event": event_name(self.event),
            "items": [i.name for i in self.items],
            "target": self.target,
            "cover_by": self.cover_by,
            "min_stay": self.min_stay,
            "amount": f"counted.{self.cover_by}" if self.min_stay is not None else self.cover_by,
        }
