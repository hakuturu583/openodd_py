"""An ODD in Python: its attributes with their probes, its modules, its verdicts.

Judging, bucketing and working out which buckets lie outside the ODD happen
in the Rust core (``openodd._core``); this module keeps what only Python can
hold -- the probes, callables reading a backend's *context* -- and gives the
core plain values.

* An **attribute** is a taxonomy concept that can be measured. Its **probe**
  reads the value from the context (``None``: missing); its **buckets** make it
  a cover item.
* A **module** holds when its include section holds and its exclude does not.
  A condition tests one attribute (``speed.between(0, 60)``), groups others
  (:func:`all_of`, :func:`any_of`), or refers to a module or a label
  (:func:`module_holds`).
* The ODD holds when its root modules hold. Missing values follow OpenODD's
  missing-value semantics: they do not by themselves put a situation outside
  the ODD; a verdict resting on them is *assumed*.
"""

from __future__ import annotations

import enum
import json
import logging
from collections.abc import Callable, Iterable, Mapping, Sequence
from dataclasses import dataclass, field
from typing import Any, Optional, Union

from . import _core
from .items import CoverGroup, CoverItem, SamplingEvent

__all__ = [
    "UNDECIDED",
    "OddAttribute",
    "OddCondition",
    "OddDefinition",
    "OddModule",
    "OddVerdict",
    "all_of",
    "any_of",
    "module_holds",
    "read_probe",
]

logger = logging.getLogger(__name__)

Probe = Callable[[Any], Any]
#: The verdict on an inactive module (or a label only inactive modules declare).
INACTIVE = "inactive"
#: An attribute whose value is open: present, but could be anything (what a
#: map-only reader returns for a value only a run knows).
UNDECIDED: Any = _core.UNDECIDED
#: A condition of a module (built from an :class:`OddAttribute`).
OddCondition = _core.Condition


def read_probe(probe: Probe, context: Any) -> Any:
    """What *probe* reads from *context*; ``None`` (missing) when it raises."""
    try:
        return probe(context)
    except Exception:
        return None


def to_core(value: Any) -> Any:
    """A value as the core reads one: an enum by name, a NumPy scalar as Python's."""
    if value is None or value is UNDECIDED or isinstance(value, (bool, int, float, str)):
        return value
    if isinstance(value, enum.Enum):
        return value.name
    item = getattr(value, "item", None)  # numpy scalars
    if callable(item):
        try:
            return item()
        except (TypeError, ValueError):
            pass
    return value


def _core_values(values: Mapping[str, Any]) -> dict[str, Any]:
    return {k: to_core(v) for k, v in values.items()}


def all_of(conditions: Sequence[OddCondition]) -> OddCondition:
    """A condition that holds when every one of *conditions* does (``AND``)."""
    return _core.all_of(list(conditions))


def any_of(conditions: Sequence[OddCondition]) -> OddCondition:
    """A condition that holds when one of *conditions* does (``OR``)."""
    return _core.any_of(list(conditions))


def module_holds(name: str, holds: bool = True) -> OddCondition:
    """A condition on another module or a label: that it holds (or not)."""
    return _core.module_holds(name, holds)


def _missing(context: Any) -> None:
    """The probe of a concept nothing measures: always missing."""
    return None


class OddAttribute:
    """A measurable taxonomy concept.

    Args:
        name: Its name, e.g. ``"scenery.speed_limit"``; its cover item is
            ``"odd." + name``.
        probe: Reads the value from the context on every step; ``None`` is missing.
        unit: The unit the probe returns; conditions written in another unit
            are converted into it.
        range, every: Equal buckets over *range*.
        buckets: Explicit bucket edges.
        values: One bucket per value.
        text: A description for the report.
        target, cover_by, min_stay: What covering a bucket takes.

    Give at most one of *values*, *buckets* and *range*. An attribute with none
    of them is monitored (conditions may test it) but not covered.
    """

    def __init__(
        self,
        name: str,
        probe: Probe,
        *,
        unit: str = "",
        range: Optional[tuple[float, float]] = None,
        every: Optional[float] = None,
        buckets: Optional[Sequence[float]] = None,
        values: Optional[Iterable[Any]] = None,
        text: str = "",
        target: float = 1,
        cover_by: str = "hits",
        min_stay: Optional[float] = None,
    ) -> None:
        self.probe = probe
        self._buckets = {"values": values, "buckets": buckets, "range": range, "every": every}
        core_values = None if values is None else [to_core(v) for v in values]
        self._core = _core.Attribute(
            name,
            None if probe is _missing else name,
            unit=unit or "",
            text=text,
            values=core_values,
            buckets=None if buckets is None else [float(x) for x in buckets],
            range=None if range is None else (float(range[0]), float(range[1])),
            every=None if every is None else float(every),
            target=float(target),
            cover_by=cover_by,
            min_stay=min_stay,
        )
        self.target = target
        self.item = self._item(values)

    @classmethod
    def _from_core(cls, core: Any, probe: Probe) -> "OddAttribute":
        self = cls.__new__(cls)
        self.probe = probe
        self._core = core
        self.target = core.target
        self.item = self._item(None)
        return self

    def _item(self, values: Optional[Iterable[Any]]) -> Optional[CoverItem]:
        bucketing = self._core.item
        if bucketing is None:
            return None
        return CoverItem._from_bucketing(
            bucketing,
            expression=self.probe,
            values=list(values) if values is not None else None,
            event=SamplingEvent.TICK,
            group=CoverGroup.ODD,
        )

    @property
    def name(self) -> str:
        return self._core.name

    @property
    def source(self) -> tuple[str, str]:
        """What the binding named to read it: ``("probe", "speed_kph")``,
        ``("measure", "gap_m")``, ``("derived", "")``, ``("code", name)`` or
        ``("missing", "")``."""
        return self._core.probe

    @property
    def unit(self) -> str:
        return self._core.unit

    @property
    def text(self) -> str:
        return self._core.text

    @property
    def cover_by(self) -> str:
        return self._core.cover_by

    @property
    def min_stay(self) -> Optional[float]:
        return self._core.min_stay

    # -- conditions --------------------------------------------------------

    def is_in(self, values: Iterable[Any]) -> OddCondition:
        """The value is one of *values* (an OpenODD list expression)."""
        return _core.is_in(self.name, [to_core(v) for v in values])

    def equals(self, value: Any) -> OddCondition:
        """The value equals *value*."""
        return _core.equals(self.name, to_core(value))

    def between(self, low: float, high: float) -> OddCondition:
        """``low <= value <= high``: OpenODD's ``"[low .. high]"``."""
        if not low <= high:
            raise ValueError(f"{self.name}.between(): low must not exceed high")
        return _core.interval(self.name, float(low), float(high))

    def at_least(self, bound: float) -> OddCondition:
        return _core.interval(self.name, low=float(bound))

    def at_most(self, bound: float) -> OddCondition:
        return _core.interval(self.name, high=float(bound))

    def greater_than(self, bound: float) -> OddCondition:
        return _core.interval(self.name, low=float(bound), include_low=False)

    def less_than(self, bound: float) -> OddCondition:
        return _core.interval(self.name, high=float(bound), include_high=False)

    def is_unknown(self) -> OddCondition:
        """The value is missing (OpenODD's ``unknown``)."""
        return _core.is_unknown(self.name)

    def __repr__(self) -> str:
        return f"OddAttribute({self.name!r})"


def _never_called(context: Any) -> Any:
    """A situation's expression: the collector fills it from the ODD's verdict."""
    return None


class OddModule:
    """A named rule: it holds when its include section holds and its exclude does not.

    As in OpenODD, a module has at most one include section and at most one
    exclude section. *situation* covers it as a situation
    (``odd.situation.<name>``): a combination the runs should drive.
    """

    def __init__(
        self,
        name: str,
        *,
        include_and: Optional[Sequence[OddCondition]] = None,
        include_or: Optional[Sequence[OddCondition]] = None,
        exclude_and: Optional[Sequence[OddCondition]] = None,
        exclude_or: Optional[Sequence[OddCondition]] = None,
        labels: Optional[Sequence[str]] = None,
        active: bool = True,
        text: str = "",
        situation: bool = False,
        target: float = 1,
        cover_by: str = "hits",
        min_stay: Optional[float] = None,
        metadata: Optional[Mapping[str, Any]] = None,
    ) -> None:
        if not name:
            raise ValueError("OddModule: name must not be empty")
        self._core = _core.Module(
            name,
            include_and=list(include_and) if include_and else None,
            include_or=list(include_or) if include_or else None,
            exclude_and=list(exclude_and) if exclude_and else None,
            exclude_or=list(exclude_or) if exclude_or else None,
            labels=list(labels or ()),
            active=active,
            text=text,
            situation=situation,
            target=float(target),
            cover_by=cover_by,
            min_stay=min_stay,
            metadata=json.dumps(dict(metadata)) if metadata else None,
        )

    @classmethod
    def _from_core(cls, core: Any) -> "OddModule":
        self = cls.__new__(cls)
        self._core = core
        return self

    def as_situation(
        self, *, target: float = 1, cover_by: str = "hits", min_stay: Optional[float] = None
    ) -> None:
        """Cover the module as a situation (before it is given to an ODD)."""
        self._core.as_situation(target=float(target), cover_by=cover_by, min_stay=min_stay)

    @property
    def name(self) -> str:
        return self._core.name

    @property
    def labels(self) -> list[str]:
        return self._core.labels

    @property
    def active(self) -> bool:
        return self._core.active

    @property
    def text(self) -> str:
        return self._core.text

    @property
    def situation(self) -> bool:
        return self._core.situation

    @property
    def item(self) -> Optional[CoverItem]:
        bucketing = self._core.item
        if bucketing is None:
            return None
        return CoverItem._from_bucketing(
            bucketing,
            expression=_never_called,
            event=SamplingEvent.TICK,
            group=CoverGroup.SITUATION,
        )

    @property
    def metadata(self) -> dict[str, Any]:
        """The module's ``METADATA`` (OpenODD 6.4.6.3)."""
        return json.loads(self._core.metadata_json())

    def conditions(self) -> list[OddCondition]:
        return [c for _, section in self._core.sections for c in section]

    def describe(self) -> dict[str, Any]:
        return json.loads(self._core.describe_json())

    def __repr__(self) -> str:
        return f"OddModule({self.name!r})"


@dataclass
class OddVerdict:
    """The ODD's verdict on one set of values."""

    #: Whether the values are inside the ODD (missing values do not put them out).
    inside: bool
    #: Inside only because values were missing.
    assumed: bool = False
    #: Every module's verdict: ``True``, ``False``, ``None`` when it rests on
    #: missing values, or ``"inactive"``.
    modules: dict[str, Union[Optional[bool], str]] = field(default_factory=dict)


class OddDefinition:
    """An ODD: its attributes and the modules that bound it.

    Args:
        name: The ODD's name, written to coverage files and reports.
        attributes: The attributes, measured on every step.
        modules: The rules. With none, every situation is inside the ODD.
        roots: The root candidates: those no other module refers to are the
            roots. ``None`` takes every module that is not a situation.
        text: A description for the report.
    """

    def __init__(
        self,
        name: str,
        attributes: Sequence[OddAttribute],
        modules: Optional[Sequence[OddModule]] = None,
        *,
        roots: Optional[Sequence[str]] = None,
        text: str = "",
    ) -> None:
        if not name:
            raise ValueError("OddDefinition: name must not be empty")
        self.attributes = list(attributes)
        self.modules = list(modules or ())
        self._core = _core.Odd(
            name,
            [a._core for a in self.attributes],
            [m._core for m in self.modules],
            roots=list(roots) if roots is not None else None,
            text=text,
        )
        self._setup()

    @classmethod
    def _from_core(cls, core: Any, probes: Mapping[str, Probe]) -> "OddDefinition":
        self = cls.__new__(cls)
        self._core = core
        self.attributes = [
            OddAttribute._from_core(a, probes.get(a.name, _missing)) for a in core.attributes
        ]
        self.modules = [OddModule._from_core(m) for m in core.modules]
        self._setup()
        return self

    def _setup(self) -> None:
        self._plain = [a for a in self.attributes if self._kind(a) != "derived"]
        self._has_derived = len(self._plain) != len(self.attributes)

    @staticmethod
    def _kind(attribute: OddAttribute) -> str:
        return attribute._core.probe[0]

    # -- what it is ----------------------------------------------------------

    @property
    def name(self) -> str:
        return self._core.name

    @property
    def text(self) -> str:
        return self._core.text

    @property
    def roots(self) -> list[str]:
        return self._core.roots

    @property
    def revision(self) -> Optional[str]:
        """The digest of the files the ODD was read from (see ``docs/versioning.md``);
        ``None`` for an ODD built in code."""
        return self._core.revision

    @property
    def version(self) -> Optional[str]:
        """What the root modules declare as ``version`` in their ``METADATA``."""
        return self._core.version

    @property
    def structure_digest(self) -> str:
        """A digest of what the ODD is once built, whatever it was read from."""
        return self._core.structure_digest

    @property
    def sources(self) -> list[dict[str, str]]:
        """The git sources read, with the commits their revisions named."""
        return json.loads(self._core.sources_json())

    @property
    def files(self) -> list[dict[str, Any]]:
        """The files read: ``{"name", "sha256", "text"}`` (and ``origin`` from git)."""
        out = []
        for name, sha, text, origin in self._core.files():
            entry: dict[str, Any] = {"name": name, "sha256": sha, "text": text}
            if origin:
                entry["origin"] = json.loads(origin)
            out.append(entry)
        return out

    def manifest(self) -> Optional[dict[str, Any]]:
        """The transmission's manifest (``None`` for an ODD built in code)."""
        text = self._core.manifest_json()
        return None if text is None else json.loads(text)

    def archive(self) -> Optional[bytes]:
        """The transmission as a deterministic ``.tar.gz`` (``load_odd_archive`` reads it)."""
        return self._core.archive()

    # -- evaluation ------------------------------------------------------------

    def evaluate(self, values: Mapping[str, Any]) -> OddVerdict:
        """The ODD's verdict on *values*, by attribute name (``None``: missing)."""
        inside, assumed, modules = self._core.evaluate(_core_values(values))
        return OddVerdict(inside, assumed, modules)

    def sample(self, context: Any) -> dict[str, Any]:
        """Every attribute's value in *context*, by name; ``None`` when missing.

        Each probe runs once; an attribute derived from others is worked out
        from their values.
        """
        values = {a.name: read_probe(a.probe, context) for a in self._plain}
        if self._has_derived:
            values = self._core.complete(_core_values(values))
        return values

    def complete(self, values: Mapping[str, Any]) -> dict[str, Any]:
        """*values* with every derived attribute worked out."""
        return self._core.complete(_core_values(values))

    # -- buckets inside and outside --------------------------------------------

    def outside_buckets(self, attribute: Union[OddAttribute, str]) -> list[str]:
        """The labels of *attribute*'s buckets that lie outside the ODD."""
        name = attribute if isinstance(attribute, str) else attribute.name
        return self._core.outside_buckets(name)

    def cover_items(self) -> list[CoverItem]:
        """The cover items of the attributes that have buckets."""
        return [a.item for a in self.attributes if a.item is not None]

    def situations(self) -> list[OddModule]:
        """The modules covered as situations."""
        return [m for m in self.modules if m.situation]

    def unmeasured(self) -> list[str]:
        """Attributes with no buckets: monitored, but not covered."""
        return self._core.unmeasured()

    def describe(self) -> dict[str, Any]:
        """The ODD's definition, as written to a coverage file."""
        out = json.loads(self._core.definition_json())
        out["structure"] = self.structure_digest
        if self.revision is not None:
            out["revision"] = self.revision
        if self.version is not None:
            out["version"] = self.version
        if self.sources:
            out["sources"] = self.sources
        return out

    def __repr__(self) -> str:
        return f"OddDefinition({self.name!r})"
