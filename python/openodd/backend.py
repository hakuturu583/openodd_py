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
road's grip -- a backend may write as rules (:mod:`.rules`), conditional
distributions: its own
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
        rules: The rules the backend derives quantities with (:mod:`.rules`):
            conditional distributions, if any.
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
    #: A scenario's nodes, added to the shared network (:meth:`add_rules`):
    #: rules, or the file they are read from.
    _rules_added: Union[Rules, str, None] = field(
        default=None, init=False, repr=False, compare=False
    )
    #: The shared rules and the added, joined (:attr:`active_rules`).
    _rules_joined: Optional[tuple[Optional[Rules], Rules]] = field(
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
        :attr:`rules` -- the shared network -- with a scenario's nodes added
        (:meth:`add_rules`)."""
        shared = self._rules_in_use if self._rules_in_use is not None else self.rules
        if self._rules_added is None:
            return shared
        joined = self._rules_joined
        if joined is None or joined[0] is not shared:
            joined = (shared, self._join(shared, self._rules_added))
            self._rules_joined = joined
        return joined[1]

    def _join(self, shared: Optional[Rules], added: Union[Rules, str]) -> Rules:
        """*shared* with *added* (rules, or a rule file) added, checked
        against :attr:`rule_inputs`."""
        if shared is None:
            joined = added if isinstance(added, Rules) else Rules.read(added)
        elif isinstance(added, Rules):
            joined = shared.extended(added)
        else:
            joined = shared.read_extending(added)
        if self.rule_inputs:
            joined.check_inputs(self.rule_inputs)
        return joined

    def add_rules(
        self, rules: Union[str, "os.PathLike[str]", Rules, None]
    ) -> Optional[Rules]:
        """Add a scenario's nodes -- a rule file (a path or an ``https://``
        URL), or rules -- to the shared network (:meth:`.Rules.extended`):
        what the scenario sets up, such as the gap to a vehicle about to cut
        in, which may read the shared nodes. A file is checked joined to the
        shared rules (:meth:`.Rules.read_extending`), and joined again to a
        run's rules (:meth:`use_rules`). ``None`` drops them. Returns the
        rules followed (:attr:`active_rules`).

        Raises:
            RulesError: When the file cannot be read, it defines a node the
                shared rules define (see :meth:`.Rules.extended`), or the
                joined rules read an input not in :attr:`rule_inputs`.
        """
        if rules is None:
            self._rules_added = None
            self._rules_joined = None
            return self.active_rules
        added: Union[Rules, str] = rules if isinstance(rules, Rules) else str(rules)
        shared = self._rules_in_use if self._rules_in_use is not None else self.rules
        # Refused now, not at the first draw.
        self._rules_joined = (shared, self._join(shared, added))
        self._rules_added = added
        return self._rules_joined[1]

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
            if self._rules_added is not None:
                # The scenario's nodes still add to the shared rules alone;
                # if they do not, nothing changes.
                self._rules_joined = (self.rules, self._join(self.rules, self._rules_added))
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
        if self._rules_added is not None:
            # The scenario's nodes still add to it.
            self._rules_joined = (followed, self._join(followed, self._rules_added))
        self._rules_in_use = followed
        return self._rules_in_use

    def rule_values(self, values: Mapping[str, Any]) -> dict[str, Any]:
        """What the rules read of *values*, the inputs a caller has: by
        default those -- a number as a float, anything else (a categorical
        input's label: a string, a bool, an enum) as given. A backend whose rules read something derived from them (the
        larger of two, say) overrides it."""
        return {
            str(k): float(v) if isinstance(v, (int, float)) and not isinstance(v, bool) else v
            for k, v in values.items()
        }

    def derived(self, quantity: str, values: Mapping[str, Any]) -> dict[str, float]:
        """``low``, ``high``, ``median`` and ``mean`` of *quantity* at
        *values*, by :attr:`active_rules`.

        Raises:
            RulesError: When the backend has no rules, or they cannot give
                *quantity* at *values* (:meth:`.Rules.evaluate`).
        """
        rules = self.active_rules
        if rules is None:
            raise RulesError(f"the {self.name} backend has no rules")
        return rules.evaluate(quantity, self.rule_values(values))

    def derived_range(
        self, quantity: str, values: Mapping[str, Any]
    ) -> tuple[float, float]:
        """``(low, high)``: the least and greatest value of *quantity* at
        *values* (:meth:`derived`)."""
        rules = self.active_rules
        if rules is None:
            raise RulesError(f"the {self.name} backend has no rules")
        return rules.range(quantity, self.rule_values(values))

    def derived_at(
        self,
        quantity: str,
        values: Mapping[str, Any],
        quantile: float,
        quantiles: Optional[Mapping[str, float]] = None,
    ) -> float:
        """The value of *quantity* at *quantile* (0 to 1) at *values*: what a
        run that drew *quantile* sets (:meth:`.Rules.quantile`). A quantity it
        reads is at its own quantile in *quantiles* (the median when not
        named)."""
        rules = self.active_rules
        if rules is None:
            raise RulesError(f"the {self.name} backend has no rules")
        return rules.quantile(
            quantity, self.rule_values(values), quantile, quantiles
        )

    def derived_sample(
        self,
        values: Mapping[str, Any],
        quantiles: Optional[Mapping[str, float]] = None,
    ) -> dict[str, float]:
        """Every quantity of :attr:`active_rules` at *values*, each at its own
        quantile in *quantiles* (:meth:`.Rules.sample`)."""
        rules = self.active_rules
        if rules is None:
            raise RulesError(f"the {self.name} backend has no rules")
        return rules.sample(self.rule_values(values), quantiles)


    # -- what the backend says of itself ----------------------------------------

    def manifest(self) -> dict[str, Any]:
        """The backend, described for tools that do not run it.

        A plain mapping (write it as YAML or JSON): the probes a binding may
        name, with their units; the reader keys (``measure``); each knob, by
        the probe whose attribute it sets, with the values it writes per
        bucket label; the inputs the rules may read, each with the knob whose
        written value it is and, for version 3 rules, the taxonomy concept it
        stands for (``concept``, ``unit``: a cause); and the rule files
        followed, with their SHA-256, and their taxonomy.
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
        concepts = self.rules.input_concepts if self.rules is not None else {}
        for name in self.rule_inputs:
            entry = {}
            if name in self.rule_input_knobs:
                entry["knob"] = self.rule_input_knobs[name]
            if name in self.rule_input_text:
                entry["text"] = self.rule_input_text[name]
            # A cause: the taxonomy concept it stands for (version 3 rules).
            if name in concepts:
                entry["concept"], unit = concepts[name]
                if unit:
                    entry["unit"] = unit
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
            if self.rules.taxonomy is not None:
                out["taxonomy"] = {"file": self.rules.taxonomy[0], "sha256": self.rules.taxonomy[1]}
        return out


def _first_line(text: Optional[str]) -> str:
    return (text or "").strip().splitlines()[0].strip() if (text or "").strip() else ""


def _knob_manifest(knob: "OddKnob") -> dict[str, Any]:
    out: dict[str, Any] = {"key": knob.key}
    if knob.quantity is not None:
        out["quantity"] = knob.quantity
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
