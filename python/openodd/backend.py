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
"""

from __future__ import annotations

import importlib
from collections.abc import Callable, Mapping
from dataclasses import dataclass, field
from typing import TYPE_CHECKING, Any, Optional

if TYPE_CHECKING:
    from .model import OddAttribute
    from .sampler import OddKnob

__all__ = [
    "Backend",
    "Probe",
    "default_backend",
    "import_callable",
    "set_default_backend",
]

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
    """

    name: str = "default"
    probes: Mapping[str, ProbeSpec] = field(default_factory=dict)
    readers: Mapping[str, Callable[[str], ProbeSpec]] = field(default_factory=dict)
    knobs: Mapping[Any, "OddKnob"] = field(default_factory=dict)
    knob_for: Optional[
        Callable[["OddAttribute", Mapping[str, "OddKnob"]], Optional["OddKnob"]]
    ] = None
    allow_imports: bool = True

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


_DEFAULT = Backend()


def default_backend() -> Backend:
    """The backend used when none is given: set by the application at start."""
    return _DEFAULT


def set_default_backend(backend: Backend) -> Backend:
    """Make *backend* the default; return the one it replaces."""
    global _DEFAULT
    previous, _DEFAULT = _DEFAULT, backend
    return previous
