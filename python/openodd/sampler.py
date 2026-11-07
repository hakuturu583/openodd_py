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
first cases at the situations still uncovered. ``weights`` (``strategy="weighted"``)
gives a share of the cases to each of some modules -- ``{"heavy_rain": 0.3}``
puts 30% of them in module ``heavy_rain`` -- and draws the rest as the uniform
strategy would. A batch of samplers on one ODD goes on where the last left off
(:class:`OddDraws`), the shares counting what the earlier ones drew.
"""

from __future__ import annotations

import dataclasses
import logging
import math
import random
from collections.abc import Iterable, Mapping, Sequence
from dataclasses import dataclass, field
from typing import Any, Optional, Union

from . import _core
from .backend import Backend, default_backend
from .items import value_label
from .model import OddDefinition, read_probe, to_core
from .report import CoverageReport

__all__ = ["OddDraws", "OddKnob", "OddSample", "OddSampler", "knobs_from_mapping", "quantile_name"]

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
            write, or ``[low, high)`` to draw it from -- a table of numbers
            the sampler turns into a rule (docs/rules.md, "A knob's values
            table is a rule"); for a numeric one, a range to draw from
            instead of the bucket's interval.
        scale, offset: A numeric value, drawn in the attribute's unit, is
            written as ``value * scale + offset``.
        integer: Write an integer.
        range: For a numeric attribute, ``[low, high]`` in its unit: what the
            run can bring about.
        limits: ``[low, high]`` the written value must lie in (``None``: open).
        quantity: A quantity of the backend's rules whose value is written
            instead of the attribute's: the setting is an effect of the
            condition drawn, given by the rules (docs/rules.md, "Knobs from
            the rules"). The quantity reads the attribute's value through the
            rule input that stands for it (``concept:``, version 3), and is
            drawn at a quantile of its own per case. It is written as
            ``value * scale + offset`` (a relative speed written as the
            absolute one, say), then held to ``limits`` and rounded by
            ``integer``; ``values`` is not used.
            A knob with a quantity that sets no attribute -- one of the
            sampler's ``effects``, or a control no attribute reads -- writes
            the quantity at whatever of its inputs the case drew (docs/rules.md,
            "Effects a scenario sets").
    """

    key: str
    values: Mapping[str, KnobValue] = field(default_factory=dict)
    scale: float = 1.0
    offset: float = 0.0
    integer: bool = False
    range: Optional[tuple[float, float]] = None
    limits: Optional[tuple[float, float]] = None
    quantity: Optional[str] = None

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


def table_rule_name(name: str) -> str:
    """The name a knob's key, or the attribute its table reads, takes in the
    rules of the knobs' values tables: every character but a letter, a digit
    or ``_`` becomes ``_`` (``environment.precipitation`` ->
    ``environment_precipitation``)."""
    out = "".join(c if c.isalnum() or c == "_" else "_" for c in name)
    return out if out and not out[0].isdigit() else f"_{out}"


def _knob_values_part(attribute: Any, knob: OddKnob) -> Optional[dict[str, Any]]:
    """The rule a knob's ``values`` table is (docs/rules.md, "A knob's values
    table is a rule"): for a categorical attribute whose table gives each
    label a number or a ``[low, high)`` range, a part reading the attribute's
    label, with a uniform distribution at each -- ``None`` for any other
    knob (a numeric attribute's ranges replace the interval drawn; a table
    of labels writes no number)."""
    item = getattr(attribute, "item", None)
    if knob.quantity is not None or not knob.values or item is None or item.values is None:
        return None
    low: list[float] = []
    high: list[float] = []
    labels: list[str] = []
    for label in item.labels:
        if label not in knob.values:
            continue  # not drawn
        v = knob.values[label]
        if _is_range(v):
            a, b = _range(v)
        elif isinstance(v, (int, float)) and not isinstance(v, bool):
            a = b = float(v)
        else:
            return None
        labels.append(label)
        low.append(a)
        high.append(b)
    if not labels:
        return None
    return {
        "input": {"labels": list(item.labels)},
        "labels": labels,
        "uniform": {"low": low, "high": high},
    }


def table_inputs(attributes: Iterable[str], quantities: Iterable[str]) -> dict[str, str]:
    """Attribute -> the input the rules of the knobs' values tables read it
    by: its name as a rule name (``table_rule_name``), with ``_label``
    appended while that names one of the tables' *quantities* -- an
    attribute ``rain`` whose knob's key is ``rain`` too."""
    taken = set(quantities)
    out: dict[str, str] = {}
    for attribute in attributes:
        name = table_rule_name(attribute)
        while name in taken:
            name += "_label"
        taken.add(name)
        out[attribute] = name
    return out


def _knob_rules(
    parts: Mapping[str, tuple[str, dict[str, Any]]], inputs: Mapping[str, str]
) -> Optional[Any]:
    """The rules of the knobs' ``values`` tables: quantity (the knob's key)
    -> (the attribute it reads, its part), each attribute read by its input
    in *inputs*."""
    import json  # noqa: PLC0415
    import warnings  # noqa: PLC0415

    from .rules import Rules  # noqa: PLC0415

    if not parts:
        return None
    doc = {
        "version": 2,
        "basis": {"kind": "expert", "by": "knob values", "text": "A knob's values table, as written."},
        "inputs": {inputs[a]: spec["input"] for a, spec in parts.values()},
        "quantities": {
            quantity: {
                "parts": {"values": {"at": {inputs[a]: spec["labels"]}, "uniform": spec["uniform"]}}
            }
            for quantity, (a, spec) in parts.items()
        },
    }
    with warnings.catch_warnings():
        # Its inputs are the ODD's attributes, not a taxonomy's concepts: a
        # table the backend wrote, not rules a reader has to classify.
        warnings.simplefilter("ignore")
        return Rules.from_text(json.dumps(doc), "knob values")


def _rule_input(backend: Backend, attribute: str, knob: OddKnob, rules: Any = None) -> str:
    """The rule input through which *knob*'s quantity reads *attribute*: the
    one that stands for it (``concept:``), else one of its name."""
    rules = rules if rules is not None else backend.active_rules
    where = f"knob {knob.key} ({attribute})"
    if rules is None:
        raise ValueError(f"{where}: takes quantity {knob.quantity!r} from the rules, and the backend has none")
    if knob.quantity not in rules.quantities:
        raise ValueError(f"{where}: the rules give no quantity {knob.quantity!r}")
    reads = set(rules.free_inputs)
    for name, (concept, _unit) in rules.input_concepts.items():
        if concept == attribute and name in reads:
            return name
    if attribute in reads:
        return attribute
    raise ValueError(
        f"{where}: no input of the rules stands for {attribute} (give one concept: {attribute})"
    )


def quantile_name(quantity: str) -> str:
    """The case parameter that holds an effect's quantile: ``quantile(q)``,
    in ``[0, 1]``. It is drawn with each case and moved by the flaky search
    (:class:`~.flaky.FlakySearch`), and the effect is written at it."""
    return f"quantile({quantity})"


def _effect_inputs(
    backend: Backend, odd: OddDefinition, effects: Sequence[OddKnob], drawn: Iterable[str] = ()
) -> dict[str, tuple[str, str, str]]:
    """Rule input -> (the ODD attribute it stands for, the attribute's unit,
    the input's unit): the inputs *effects*' quantities read, through the
    quantities they read, that a case draws (*drawn*: the attributes given a
    knob). Every other input they read must have a default."""
    if not effects:
        return {}
    rules = backend.active_rules
    for knob in effects:
        if knob.quantity is None:
            raise ValueError(f"effect {knob.key}: writes no quantity (give quantity:)")
        if rules is None:
            raise ValueError(f"effect {knob.key}: takes quantity {knob.quantity!r} from the rules, and the backend has none")
        if knob.quantity not in rules.quantities:
            raise ValueError(f"effect {knob.key}: the rules give no quantity {knob.quantity!r}")
    assert rules is not None
    # What the quantities read, through the quantities they read.
    seen: set[str] = set()
    stack = [str(k.quantity) for k in effects]
    reads: set[str] = set()
    while stack:
        q = stack.pop()
        if q in seen:
            continue
        seen.add(q)
        for name in rules.inputs(q):
            if name in rules.quantities:
                stack.append(name)
            else:
                reads.add(name)
    drawn = set(drawn)
    attributes = {a.name: a for a in odd.attributes if a.name in drawn}
    concepts = rules.input_concepts
    defaults = rules.input_defaults
    out: dict[str, tuple[str, str, str]] = {}
    for name in sorted(reads):
        concept, input_unit = concepts.get(name, (name, ""))
        attribute = attributes.get(concept)
        if attribute is not None:
            out[name] = (attribute.name, attribute.unit, input_unit or attribute.unit)
        elif defaults.get(name) is None:
            raise ValueError(
                f"effects: the rules read {name}, which stands for no attribute of ODD {odd.name} "
                "the sampler draws (one given a knob) and has no default"
            )
    return out


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
        unknown = set(spec) - {"key", "values", "scale", "offset", "integer", "range", "limits", "quantity"}
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
            quantity=None if spec.get("quantity") is None else str(spec["quantity"]),
        )
    return knobs


@dataclass(frozen=True)
class OddSample:
    """One concrete case the sampler drew.

    Attributes:
        index: 0-based position in the batch.
        overrides: The settings that bring it about, ``key=value`` each.
        buckets: Attribute name -> the bucket label drawn for it.
        values: Attribute name -> the value drawn, in the attribute's terms;
            and each effect's quantile, ``quantile(q)`` (:func:`quantile_name`).
        situation: The situation the case was aimed at, if any.
        flaky: Its flakiness, when :class:`~.flaky.FlakySearch` chose it for
            that (:class:`~.flaky.FlakyMetrics`); ``None`` for a case the
            sampler's own policy drew.
        quantities: Rules quantity -> the value a knob wrote from the rules
            (``OddKnob(quantity=...)``), after its limits and rounding: the effects the
            case ran with, which are no attributes of the ODD.
    """

    index: int
    overrides: list[str]
    buckets: dict[str, str]
    values: dict[str, Any]
    situation: Optional[str] = None
    flaky: Optional[Any] = None
    quantities: dict[str, float] = field(default_factory=dict)

    @classmethod
    def _from_core(cls, core: Any, flaky: Optional[Any] = None) -> "OddSample":
        return cls(
            core.index, list(core.overrides), dict(core.buckets), dict(core.values), core.situation, flaky
        )

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


def _coverage_core(coverage: Any) -> Any:
    """The core's report: a :class:`CoverageReport`, or anything with ``entries``
    that have a ``name``, ``buckets``, ``target``, ``amount(bucket)`` and
    (optionally) ``cover_by``."""
    if coverage is None:
        return None
    core = getattr(coverage, "_core", None)
    if core is not None:
        return core
    return _core.coverage_from_entries([
        (
            e.name,
            list(e.buckets),
            float(e.target),
            str(getattr(e, "cover_by", "hits")),
            {b: float(e.amount(b)) for b in e.buckets},
        )
        for e in coverage.entries
    ])


def _write(
    knob: OddKnob, quantity: str, y: float, quantities: dict[str, Any], overrides: list[str]
) -> list[str]:
    """*overrides* with *knob* writing *quantity*'s value *y* (scaled, held
    within its limits and rounded), in the place of the knob's own override
    when the case has one; *quantities* records what the case ran with: the
    quantity the value written stands for (the first knob's, when several
    write it)."""
    v = y * knob.scale + knob.offset
    if knob.limits is not None:
        v = min(max(v, knob.limits[0]), knob.limits[1])
    written = str(int(round(v))) if knob.integer else repr(float(v))
    ran = (float(written) - knob.offset) / knob.scale if knob.scale else y
    quantities.setdefault(quantity, ran)
    prefix = f"{knob.key}="
    if not any(o.startswith(prefix) for o in overrides):
        return [*overrides, prefix + written]
    return [prefix + written if o.startswith(prefix) else o for o in overrides]


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
        strategy: ``"uniform"``, ``"coverage"`` or ``"weighted"``; ``"weighted"``
            when *weights* are given, else ``"uniform"``, if not given.
        coverage: The coverage of earlier runs (:func:`~.report.load_and_merge`),
            read by the ``coverage`` strategy.
        weights: Module -> the share of the cases in it, in ``[0, 1]``, for
            the ``weighted`` strategy. Modules may overlap: each holds its
            share of the batch (as closely as whole cases allow), and inside
            and outside them the cases are drawn as the uniform strategy
            draws them. A module must not be a root of the ODD, and whether a
            case is in it must follow from what the knobs set.
        max_tries: Combinations drawn per case before giving up on it.
        drawn: The cases earlier samplers of the batch drew from this ODD
            (:meth:`OddDraws.of`).
        backend: Whose knobs (the default backend if none).
        effects: Knobs that write a quantity of the backend's rules and set no
            attribute: effects of the network a scenario sets up (the gap to
            a vehicle ahead). Each case draws every quantity at a quantile of
            its own, at the values drawn for the inputs that stand for the
            ODD's attributes, and writes it (:attr:`OddSample.quantities`). A
            control with a ``quantity`` that no attribute reads is one too.
            The quantile is a parameter of the case, ``quantile(q)`` in its
            ``values`` (:func:`quantile_name`): continuous, it is what the
            flaky search moves.
    """

    def __init__(
        self,
        odd: OddDefinition,
        knobs: Optional[Mapping[str, OddKnob]] = None,
        *,
        controls: Optional[Mapping[str, OddKnob]] = None,
        seed: Union[int, str] = 0,
        strategy: Optional[str] = None,
        coverage: Optional[CoverageReport] = None,
        max_tries: int = 200,
        drawn: Sequence[OddSample] = (),
        backend: Optional[Backend] = None,
        weights: Optional[Mapping[str, float]] = None,
        effects: Sequence[OddKnob] = (),
    ) -> None:
        backend = backend or default_backend()
        backend.check_supported(odd)
        knobs = dict(knobs or {})
        controls = dict(controls or {})
        chosen: list[tuple[str, Any]] = []
        names = {a.name for a in odd.attributes}
        unknown = sorted(set(knobs) - names)
        if unknown:
            raise ValueError(f"OddSampler: ODD {odd.name} has no attributes {unknown}")
        # Attribute -> (knob, the rule input reading it, the attribute's unit,
        # the input's unit): the value drawn is converted between them.
        self._from_rules: dict[str, tuple[OddKnob, str, str, str]] = {}
        # The controls the ODD's attributes read, whichever knob is chosen
        # (an explicit knob replaces one, and it is still the attribute's,
        # not an effect).
        used: set[int] = set()
        active = backend.active_rules
        # A knob's values table is a rule (_knob_values_part): quantity (the
        # knob's key) -> (the attribute it reads, its part). A quantity of that
        # name in the backend's rules -- a table measured, read over them --
        # is the one followed instead.
        table_parts: dict[str, tuple[str, dict[str, Any]]] = {}
        # Attribute -> (the knob writing its table's quantity, the input the
        # rule reads the label by), for the tables the knob rules follow.
        self._tables: dict[str, tuple[OddKnob, str]] = {}
        resolved: list[tuple[Any, OddKnob]] = []
        for attribute in odd.attributes:
            control = backend.knob(attribute, controls)
            if control is not None:
                used.add(id(control))
            knob = knobs.get(attribute.name) or control
            if knob is None:
                continue
            part = _knob_values_part(attribute, knob)
            if part is not None:
                quantity = table_rule_name(knob.key)
                # The core draws the label among those the table gives (its
                # draws as they always were); the rule writes the value.
                chosen.append((attribute.name, knob._core()))
                knob = dataclasses.replace(
                    knob, values={}, scale=1.0, offset=0.0, quantity=quantity, range=None
                )
                if active is not None and quantity in active.quantities:
                    resolved.append((attribute, knob))
                else:
                    # Keys that read the same as a rule name (env.rain,
                    # env_rain) each keep a quantity of their own.
                    n = 2
                    while quantity in table_parts:
                        quantity = f"{table_rule_name(knob.key)}_{n}"
                        n += 1
                    knob = dataclasses.replace(knob, quantity=quantity)
                    table_parts[quantity] = (attribute.name, part)
                    self._tables[attribute.name] = (knob, "")
                continue
            # A knob that writes a quantity of the rules sets the attribute
            # only through them: the core draws the attribute over its
            # range alone, and limits and rounding act on the value
            # written (_realize), not on what is drawn.
            core = knob if knob.quantity is None else OddKnob(knob.key, range=knob.range)
            chosen.append((attribute.name, core._core()))
            if knob.quantity is not None:
                resolved.append((attribute, knob))
        #: The rules of the knobs' values tables (``None``: no table is one).
        #: Each reads only its attribute's label, so they are worked out
        #: apart from the backend's rules, before them.
        inputs_of = table_inputs(self._tables, table_parts)
        self._tables = {a: (k, inputs_of[a]) for a, (k, _) in self._tables.items()}
        self.knob_rules = _knob_rules(table_parts, inputs_of)
        for attribute, knob in resolved:
            rule_input = _rule_input(backend, attribute.name, knob, active)
            input_unit = active.input_concepts.get(rule_input, ("", ""))[1] if active else ""
            self._from_rules[attribute.name] = (knob, rule_input, attribute.unit, input_unit)
        # Effects: knobs that write a quantity and set no attribute.
        self._effects = list(effects) + [
            k for k in controls.values() if k.quantity is not None and id(k) not in used
        ]
        self._effect_inputs = _effect_inputs(backend, odd, self._effects, (n for n, _ in chosen))
        if not chosen and not self._effects:
            raise ValueError(
                f"OddSampler: nothing of ODD {odd.name} can be drawn: no attribute is given a knob, "
                "and there are no effects"
            )
        self._rules = active if self._from_rules or self._effects else None
        self._seed = str(seed)
        if strategy is None:
            strategy = "weighted" if weights is not None else "uniform"
        self.odd = odd
        self.strategy = strategy
        derived = list(getattr(odd, "_code_derived", ()))

        def derive(values: dict[str, Any]) -> dict[str, Any]:
            return {a.name: read_probe(a.probe.from_values, values) for a in derived}

        self._core = _core.Sampler(
            odd._core,
            chosen,
            seed=str(seed),
            strategy=strategy,
            coverage=_coverage_core(coverage),
            max_tries=max_tries,
            drawn=[s._core() for s in drawn],
            derive=derive if derived else None,
            weights=None if weights is None else [(str(k), float(v)) for k, v in weights.items()],
            free=[quantile_name(q) for q in [*self.effects, *self.table_quantities]] or None,
        )

    @property
    def attributes(self) -> list[str]:
        """The attributes drawn, in the ODD's order."""
        return self._core.attributes

    @property
    def coverage_amounts(self) -> dict[tuple[str, str], float]:
        """(attribute, bucket) -> how many times over it is covered, as read from
        the coverage given (only what counts what the ODD counts)."""
        return dict(self._core.coverage_amounts)

    @property
    def weights(self) -> dict[str, float]:
        """Module -> the share of the cases in it (the ``weighted`` strategy)."""
        return dict(self._core.weights)

    @property
    def situation_holes(self) -> list[str]:
        """The situations still uncovered, which the first cases are aimed at."""
        return self._core.situation_holes

    @property
    def unreachable(self) -> list[tuple[str, str]]:
        """``(attribute, bucket)`` pairs the knobs can set but no case inside
        the ODD has, given up on as holes by the coverage strategy on its first
        draw."""
        return self._core.unreachable

    def _log_warnings(self) -> None:
        """Log what went wrong but did not stop the draws."""
        for warning in self._core.take_warnings():
            logger.warning(warning)

    def draw(self, index: int = 0, situation: Optional[str] = None) -> OddSample:
        """Draw one case; *situation*, if given, is a module the case aims at."""
        case = self._realize(OddSample._from_core(self._core.draw(index, situation)))
        self._log_warnings()
        return case

    def sample(self, count: int) -> list[OddSample]:
        """Draw *count* cases; with the coverage strategy, the first aim at the
        situations still uncovered, one each; with the weighted strategy, each
        module given a share holds it."""
        if count < 0:
            raise ValueError("OddSampler: count must not be negative")
        cases = [self._realize(OddSample._from_core(c)) for c in self._core.sample(count)]
        self._log_warnings()
        return cases

    @property
    def table_quantities(self) -> list[str]:
        """The quantities of the knobs' values tables (``knob_rules``), in
        the ODD's order: each drawn at a quantile of its own per case, a
        parameter of the case (``quantile(q)``) like an effect's."""
        return [str(k.quantity) for k, _ in self._tables.values()]

    @property
    def effects(self) -> list[str]:
        """The quantities the sampler's effects write, in the rules' order."""
        wanted = {str(k.quantity) for k in self._effects}
        return [q for q in self._rules.order if q in wanted] if self._rules is not None else []

    def _realize(self, case: OddSample) -> OddSample:
        """*case* with every knob that takes its value from the rules writing
        it: a knob on an attribute, the quantity at the attribute's drawn
        value; an effect, its quantity at the case's inputs.

        One pass over the quantities in the rules' order, so a quantity reads
        whatever it depends on at the value written to the run (after that
        knob's limits and rounding), whichever kind of knob wrote it. A
        quantity no knob writes is read at its own quantile, the same in
        every evaluation of the case."""
        if not self._tables and ((not self._from_rules and not self._effects) or self._rules is None):
            return case
        overrides = list(case.overrides)
        quantities = dict(case.quantities)
        # The knobs' values tables first: each reads its attribute's label
        # alone, at a quantile of its own (from the attribute's name).
        for name, (knob, rule_input) in self._tables.items():
            label = case.values.get(name)
            if label is None or self.knob_rules is None:
                continue
            q = str(knob.quantity)
            # The case's own parameter (quantile(q), drawn with it and moved
            # by the flaky search), else one from the seed.
            p = case.values.get(quantile_name(q))
            if isinstance(p, (int, float)) and not isinstance(p, bool):
                at = min(max(float(p), 0.0), 1.0)
            else:
                at = random.Random(f"{self._seed}:{case.index}:{name}").random()
            given = {rule_input: value_label(label)}
            if knob.integer:
                # Among the integers in [low, high), each as likely, as the
                # core draws them.
                low, high = self.knob_rules.range(q, given)
                first = math.ceil(low)
                last = first if low == high else math.ceil(high) - 1
                y = float(first + min(int(at * (last - first + 1)), max(last - first, 0)))
            else:
                y = self.knob_rules.quantile(q, given, at)
            overrides = _write(knob, q, y, quantities, overrides)
        if (not self._from_rules and not self._effects) or self._rules is None:
            return dataclasses.replace(case, overrides=overrides, quantities=quantities)
        # The inputs that stand for the ODD's attributes, at the values drawn
        # (in the attribute's unit; the rule's points are in the input's).
        inputs: dict[str, Any] = {}
        labelled = self._rules.input_labels
        for rule_input, (attribute, unit, input_unit) in self._effect_inputs.items():
            x = case.values.get(attribute)
            if rule_input in labelled:
                # A categorical cause: the label drawn (the rules read its index).
                if x is not None:
                    inputs[rule_input] = value_label(x)
            elif isinstance(x, (int, float)) and not isinstance(x, bool):
                inputs[rule_input] = _core.convert(float(x), unit, input_unit)
        # Each quantity at a quantile of its own per case, reproducible from
        # the seed: an effect's is the case's own parameter when it has one
        # (quantile(q), drawn with it and moved by the flaky search); a knob
        # on an attribute keeps one of its own (from the attribute's name).
        quantiles = {
            q: random.Random(f"{self._seed}:{case.index}:{q}").random() for q in self._rules.order
        }
        writers: dict[str, list[OddKnob]] = {}
        for name, (knob, rule_input, unit, input_unit) in self._from_rules.items():
            x = case.values.get(name)
            if rule_input in labelled and x is not None:
                inputs[rule_input] = value_label(x)
            elif isinstance(x, (int, float)) and not isinstance(x, bool):
                inputs[rule_input] = _core.convert(float(x), unit, input_unit)
            else:
                continue
            q = str(knob.quantity)
            if q not in writers:
                quantiles[q] = random.Random(f"{self._seed}:{case.index}:{name}").random()
            writers.setdefault(q, []).append(knob)
        for q in self.effects:
            p = case.values.get(quantile_name(q))
            if q not in writers and isinstance(p, (int, float)) and not isinstance(p, bool):
                quantiles[q] = min(max(float(p), 0.0), 1.0)
            writers.setdefault(q, []).extend(k for k in self._effects if k.quantity == q)
        # A quantity already in the case is read at that value.
        inputs.update(quantities)
        for q in self._rules.order:
            if q not in writers:
                continue
            y = self._rules.quantile(q, inputs, quantiles[q], quantiles)
            for knob in writers[q]:
                overrides = _write(knob, q, y, quantities, overrides)
            inputs[q] = quantities[q]
        return dataclasses.replace(case, overrides=overrides, quantities=quantities)
