"""Backends: what measures an ODD's attributes, for the application at hand.

An OpenODD document says what an ODD *is*; how it is measured belongs to the
application. A simulator reads the speed limit from its world, a log analyser
from a column of a recorded drive, a dataset tool from a frame's tags. This
library leaves that to a :class:`Backend`, which a binding file names its
probes through::

    probes:
      speed_limit: {probe: speed_limit_kph}        # a probe the backend names
      lead_gap: {measure: vehicle_ahead_gap_m}     # a reader of the backend's
      rain: {probe: my_package.probes:rain}        # or package.module:function

A probe is any callable taking the backend's *context* (whatever a step is
measured on: a world, a log row) and returning the value, or ``None`` when it
is missing. A probe that raises is read as missing too.

The sampler (:mod:`.sampler`) asks the backend the other way round as well:
how a run *sets* an attribute (:meth:`Backend.knob`), so an application that
can stage conditions -- a simulator's weather -- can be sampled from an ODD.
The coverage collector (:mod:`.collector`) asks it where the ego is
(:meth:`Backend.position`, :meth:`Backend.geolocation`).

A backend can describe itself (:meth:`Backend.manifest`): its probes, its
knobs and the inputs its rules read, written out so that tools without the
application -- the editor in the browser -- can connect an ODD's attributes,
the binding that measures them, and the backend's rules.

What a run's settings do to the rest of it -- a simulator's rain to the
road's grip -- a backend may write as fuzzy rules (:mod:`.rules`): its own
(:attr:`Backend.rules`), which a run may read another file over
(:meth:`Backend.use_rules`), and which it asks for a quantity at what the run
set (:meth:`Backend.derived`).

An application subclasses :class:`Backend` for what is its own and leaves the
rest -- the model, the reader, coverage, the sampler -- to this library.
"""

from __future__ import annotations

import importlib
import os
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass, field
from typing import TYPE_CHECKING, Any, Optional, Union

from .rules import Rules, RulesError

if TYPE_CHECKING:
    from .model import OddAttribute
    from .sampler import OddKnob

__all__ = [
    "MANIFEST_VERSION",
    "Backend",
    "Probe",
    "default_backend",
    "import_callable",
    "set_default_backend",
]

#: The version of the format :meth:`Backend.manifest` writes.
MANIFEST_VERSION = 1

#: Reads an attribute's value from the backend's context; ``None`` is missing.
Probe = Callable[[Any], Any]
#: A probe and the unit it returns its value in (``""``: none, or unknown).
ProbeSpec = tuple[Probe, str]


def import_callable(spec: str) -> Callable[..., Any]:
    """The function ``"package.module:function"`` names.

    Raises:
        ValueError: if *spec* does not name one.
    """
    module_name, sep, attr = spec.partition(":")
    if not sep or not module_name or not attr:
        raise ValueError(f"{spec!r} is not package.module:function")
    try:
        found = getattr(importlib.import_module(module_name), attr)
    except (ImportError, AttributeError) as exc:
        raise ValueError(f"cannot import {spec!r}: {exc}") from exc
    if not callable(found):
        raise ValueError(f"{spec!r} is not callable")
    return found


@dataclass
class Backend:
    """The probes an application measures with, and how it sets attributes.

    Attributes:
        name: For messages.
        probes: Probe name (``probe:`` in a binding) -> the probe and its unit.
        readers: Binding key -> how a value under it is read: ``measure:
            vehicle_ahead_gap_m`` calls ``readers["measure"]("vehicle_ahead_gap_m")``
            for the probe and its unit. A binding names a probe or one reader.
        knobs: Probe -> how a run sets the attribute it reads (sampling).
        knob_for: Or computed: the attribute and the run's controls (a
            scenario's ``controls``, keyed as the backend likes) -> a knob,
            or ``None``. Asked before *knobs*.
        allow_imports: Whether ``probe: package.module:function`` imports.
        rules: The fuzzy rules the backend derives quantities with
            (:mod:`.rules`), if any.
        rule_inputs: The inputs those rules may read (any, if empty): what
            :meth:`rule_values` hands them.
        rule_input_knobs: Rule input -> the key of the knob whose written
            value it is (``precipitation`` -> ``environment.rain``): how an
            attribute a knob sets reaches the rules (:meth:`manifest`).
        rule_input_text: Rule input -> a line saying what it is (``water``:
            the larger of ``wetness`` and ``precipitation_deposits``).
    """

    name: str = "default"
    probes: Mapping[str, ProbeSpec] = field(default_factory=dict)
    readers: Mapping[str, Callable[[str], ProbeSpec]] = field(default_factory=dict)
    knobs: Mapping[Any, "OddKnob"] = field(default_factory=dict)
    knob_for: Optional[
        Callable[["OddAttribute", Mapping[str, "OddKnob"]], Optional["OddKnob"]]
    ] = None
    allow_imports: bool = True
    rules: Optional[Rules] = None
    rule_inputs: Sequence[str] = ()
    rule_input_knobs: Mapping[str, str] = field(default_factory=dict)
    rule_input_text: Mapping[str, str] = field(default_factory=dict)
    #: A run's rules over :attr:`rules` (:meth:`use_rules`).
    _rules_in_use: Optional[Rules] = field(
        default=None, init=False, repr=False, compare=False
    )

    def __post_init__(self) -> None:
        if self.rules is not None and self.rule_inputs:
            self.rules.check_inputs(self.rule_inputs)

    def probe(self, spec: str) -> ProbeSpec:
        """A probe and its unit, by the backend's name or ``package.module:function``.

        Raises:
            ValueError: if neither names one.
        """
        if spec in self.probes:
            return self.probes[spec]
        if ":" not in spec or not self.allow_imports:
            known = ", ".join(sorted(self.probes)) or "none"
            raise ValueError(
                f"unknown probe {spec!r}; the {self.name} backend has: {known}"
                + (", or package.module:function" if self.allow_imports else "")
            )
        probe = import_callable(spec)
        return probe, str(getattr(probe, "unit", "") or "")

    def read(self, key: str, value: str) -> ProbeSpec:
        """The probe a binding's ``key: value`` (``measure: gap_m``) reads with."""
        if key not in self.readers:
            raise ValueError(f"the {self.name} backend has no {key!r} readers")
        return self.readers[key](value)

    # -- what the collector asks on every step ---------------------------------

    def position(self, context: Any) -> Optional[tuple[float, float, float]]:
        """Where the ego is in *context*, in metres (``None``: unknown).

        The collector measures the distance a step stands for with it (an
        item's ``cover_by="meters"``). A backend that can tell overrides it.
        """
        return None

    def geolocation(self, context: Any) -> Optional[tuple[float, float]]:
        """The ego's latitude and longitude in *context* (``None``: unknown).

        Kept with every step's ODD samples, it is the spatial extent of an
        exported COD (:mod:`.cod`). A backend that can tell overrides it.
        """
        return None

    # -- what the Rust reader asks while it reads a binding --------------------

    def reader_keys(self) -> list[str]:
        """Binding keys naming one of the backend's readers."""
        return list(self.readers)

    def probe_unit(self, spec: str) -> str:
        """The unit the probe *spec* names reads in; ValueError if none is named."""
        return self.probe(spec)[1]

    def reader_unit(self, key: str, value: str) -> str:
        """The unit a reader reads *value* in; ValueError if it cannot."""
        return self.read(key, value)[1]

    def knob(
        self, attribute: "OddAttribute", controls: Mapping[str, "OddKnob"]
    ) -> Optional["OddKnob"]:
        """How a run sets *attribute*, if it can (see :class:`~.sampler.OddSampler`)."""
        if self.knob_for is not None:
            knob = self.knob_for(attribute, controls)
            if knob is not None:
                return knob
        # Read by one of the backend's readers (``measure: gap_m``): the run's
        # controls say which of its settings sets that, keyed alike.
        kind, spec = getattr(attribute, "source", ("", ""))
        if kind in self.readers and spec in controls:
            return controls[spec]
        try:
            return self.knobs.get(attribute.probe)
        except TypeError:  # an unhashable probe has no knob
            return None


    # -- what a run's settings do to the rest of it ------------------------------

    @property
    def active_rules(self) -> Optional[Rules]:
        """The rules :meth:`derived` follows: a run's (:meth:`use_rules`), else
        :attr:`rules`."""
        return self._rules_in_use if self._rules_in_use is not None else self.rules

    def use_rules(
        self, rules: Union[str, "os.PathLike[str]", Rules, None]
    ) -> Optional[Rules]:
        """Follow *rules* -- a rule file (a path or an ``https://`` URL), or
        rules -- merged over :attr:`rules` from now on (:meth:`.Rules.merged`:
        what it names replaces, down to the rule base); ``None`` goes back to
        :attr:`rules` alone. Returns the rules followed.

        Raises:
            RulesError: When the file cannot be read, its rules are not well
                formed, or they read an input not in :attr:`rule_inputs`.
        """
        if rules is None:
            self._rules_in_use = None
            return self.rules
        if isinstance(rules, Rules):
            followed = rules if self.rules is None else self.rules.merged(rules)
        elif self.rules is None:
            followed = Rules.read(rules)
        else:
            # The file need only name what it changes over the backend's.
            followed = self.rules.read_over(rules)
        if self.rule_inputs:
            followed.check_inputs(self.rule_inputs)
        self._rules_in_use = followed
        return self._rules_in_use

    def rule_values(self, values: Mapping[str, float]) -> dict[str, float]:
        """What the rules read of *values*, the inputs a caller has: by
        default those. A backend whose rules read something derived from them
        (the larger of two, say) overrides it."""
        return {str(k): float(v) for k, v in values.items()}

    def derived(self, quantity: str, values: Mapping[str, float]) -> dict[str, float]:
        """Each output of *quantity* at *values*, by :attr:`active_rules`.

        Raises:
            RulesError: When the backend has no rules, or they cannot give
                *quantity* at *values* (:meth:`.Rules.evaluate`).
        """
        rules = self.active_rules
        if rules is None:
            raise RulesError(f"the {self.name} backend has no rules")
        return rules.evaluate(quantity, self.rule_values(values))

    def derived_range(
        self, quantity: str, values: Mapping[str, float]
    ) -> tuple[float, float]:
        """``(low, high)`` of a quantity whose outputs are ``low`` and ``high``
        (:meth:`derived`)."""
        rules = self.active_rules
        if rules is None:
            raise RulesError(f"the {self.name} backend has no rules")
        return rules.range(quantity, self.rule_values(values))


    # -- what the backend says of itself ----------------------------------------

    def manifest(self) -> dict[str, Any]:
        """The backend, described for tools that do not run it.

        A plain mapping (write it as YAML or JSON): the probes a binding may
        name, with their units; the reader keys (``measure``); each knob, by
        the probe whose attribute it sets, with the values it writes per
        bucket label; the inputs the rules may read, each with the knob whose
        written value it is; and the rule files followed, with their SHA-256.
        The editor reads it to connect an ODD's attributes, the binding that
        measures them, and the rules (docs/editor.md).
        """
        names: dict[int, str] = {id(p): n for n, (p, _) in self.probes.items()}
        probes: dict[str, Any] = {}
        for name, (probe, unit) in self.probes.items():
            entry: dict[str, Any] = {"unit": unit}
            text = _first_line(getattr(probe, "__doc__", None))
            if text:
                entry["text"] = text
            probes[name] = entry
        knobs: dict[str, Any] = {}
        for key, knob in self.knobs.items():
            name = key if isinstance(key, str) else names.get(id(key))
            if name is None:
                continue
            knobs[name] = _knob_manifest(knob)
        inputs: dict[str, Any] = {}
        for name in self.rule_inputs:
            entry = {}
            if name in self.rule_input_knobs:
                entry["knob"] = self.rule_input_knobs[name]
            if name in self.rule_input_text:
                entry["text"] = self.rule_input_text[name]
            inputs[str(name)] = entry
        out: dict[str, Any] = {
            "openodd_backend": MANIFEST_VERSION,
            "name": self.name,
            "probes": probes,
            "readers": sorted(self.readers),
            "knobs": knobs,
            "rule_inputs": inputs,
        }
        if self.rules is not None:
            out["rules"] = [{"file": f, "sha256": h} for f, h in self.rules.files]
        return out


def _first_line(text: Optional[str]) -> str:
    return (text or "").strip().splitlines()[0].strip() if (text or "").strip() else ""


def _knob_manifest(knob: "OddKnob") -> dict[str, Any]:
    out: dict[str, Any] = {"key": knob.key}
    if knob.values:
        out["values"] = {
            str(label): (list(v) if isinstance(v, (list, tuple)) else v)
            for label, v in knob.values.items()
        }
    if knob.range is not None:
        out["range"] = list(knob.range)
    if knob.limits is not None:
        # An open end is None, as a knob is written: JSON has no infinity.
        out["limits"] = [None if abs(x) == float("inf") else x for x in knob.limits]
    if knob.scale != 1.0:
        out["scale"] = knob.scale
    if knob.offset != 0.0:
        out["offset"] = knob.offset
    if knob.integer:
        out["integer"] = True
    return out


_DEFAULT = Backend()


def default_backend() -> Backend:
    """The backend used when none is given: set by the application at start."""
    return _DEFAULT


def set_default_backend(backend: Backend) -> Backend:
    """Make *backend* the default; return the one it replaces."""
    global _DEFAULT
    previous, _DEFAULT = _DEFAULT, backend
    return previous
