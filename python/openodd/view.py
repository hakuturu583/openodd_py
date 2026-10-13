"""The ODD coverage matrix as a UI component for other tools' pages.

:class:`CoverageView` reads coverage documents once and renders, on the Rust
core (the ``openodd-coverage-view`` crate), a matrix of any two ODD
attributes: per pair of buckets, the runs that spent time there, how many
failed and how many failed *there*, coloured by what they did:

* yellow -- flaky: runs in it both passed and failed;
* red -- runs in it failed; green -- runs in it passed;
* grey -- outside the ODD; white -- no run was in it;
* hatched -- only runs without an outcome were in it.

A cell with too few runs to judge (``min_runs``, ``band``, as
:meth:`~.report.CellRow.status`) has a dashed outline. Clicking a cell keeps
only the time spent in it and lays the next two attributes out inside it.

Embedding::

    view = openodd.CoverageView(documents)
    head = openodd.view.styles()          # once per page, in a <style>
    body = view.html(conditions=[("environment.rain", "heavy")],
                     link="/coverage?{conditions}")   # host-driven drill-down
    tail = openodd.view.script()          # once per page, in a <script>

A cell is then a link to ``/coverage?at=attribute%3Dbucket&...``
(:func:`parse_conditions` reads it back), and a click also dispatches a
cancelable ``openodd:drill`` DOM event whose ``detail.conditions`` is the
list of ``[attribute, bucket]``. :meth:`CoverageView.explorer_html` instead
renders the levels below ahead of time, for a static report
(``openodd-coverage --html``).

:class:`CasePlot` shows the drawn cases of a sweep instead: every case at
the values it was drawn with, along any two attributes, marked by what its
runs did (``openodd-coverage --cases-html``). It shares the styles and the
script.
"""

from __future__ import annotations

import json
from collections.abc import Iterable, Mapping, Sequence
from typing import Any, Optional, Union

from . import _core

__all__ = ["ANY", "CasePlot", "CoverageView", "page", "parse_conditions", "script", "styles"]

Conditions = Union[Mapping[str, str], Iterable[Sequence[str]]]

#: Leave ``environment`` out of a query: every environment.
ANY = object()


def styles() -> str:
    """The components' CSS (:class:`CoverageView` and :class:`CasePlot`),
    scoped under ``.oo-cov``; once per page."""
    return str(_core.coverage_view_styles())


def script() -> str:
    """The components' script (drill and axes events, explorer panels,
    tooltip cards); once per page."""
    return str(_core.coverage_view_script())


def page(title: str, body: str) -> str:
    """A standalone HTML page around *body*, with the styles and the script."""
    return str(_core.coverage_view_page(title, body))


def parse_conditions(values: Iterable[str]) -> list[tuple[str, str]]:
    """``attribute=bucket`` query values (the ``at`` of a cell's link), in
    order; one per attribute. It splits at the first ``=``: an attribute
    name with ``=`` in it does not round-trip (a bucket label may hold one)."""
    out: dict[str, str] = {}
    for value in values:
        name, sep, label = str(value).partition("=")
        if sep and name and name not in out:
            out[name] = label
    return list(out.items())


def _conditions(conditions: Optional[Conditions]) -> list[list[str]]:
    if conditions is None:
        return []
    pairs = conditions.items() if isinstance(conditions, Mapping) else conditions
    out = []
    for pair in pairs:
        attribute, bucket = pair
        out.append([str(attribute), str(bucket)])
    return out


class CoverageView:
    """Coverage documents (the contents of ``*_coverage.json`` files), read
    once, as matrices and their HTML."""

    def __init__(self, documents: Sequence[Mapping[str, Any]]) -> None:
        self._core = _core.CoverageView(json.dumps(list(documents)))

    @property
    def attributes(self) -> list[str]:
        """The ODD attributes the runs' cells are of, in the order first seen."""
        return list(self._core.attributes)

    @property
    def scenarios(self) -> list[str]:
        return list(self._core.scenarios)

    @property
    def environments(self) -> list[Optional[str]]:
        """The environments of the runs with an outcome (``None``: none recorded)."""
        return list(self._core.environments)

    @property
    def runs(self) -> int:
        """Runs with ODD cells."""
        return int(self._core.runs)

    @staticmethod
    def _query(
        x: Optional[str],
        y: Optional[str],
        conditions: Optional[Conditions],
        scenario: Optional[str],
        environment: Any,
        min_runs: Optional[int],
        band: Optional[tuple[float, float]],
        min_seconds: Optional[float],
    ) -> str:
        q: dict[str, Any] = {"x": x, "y": y, "conditions": _conditions(conditions), "scenario": scenario}
        if environment is not ANY:
            q["environment"] = environment
        if min_runs is not None:
            q["min_runs"] = int(min_runs)
        if band is not None:
            q["band"] = [float(band[0]), float(band[1])]
        if min_seconds is not None:
            q["min_seconds"] = float(min_seconds)
        return json.dumps(q)

    def matrix(
        self,
        *,
        x: Optional[str] = None,
        y: Optional[str] = None,
        conditions: Optional[Conditions] = None,
        scenario: Optional[str] = None,
        environment: Any = ANY,
        min_runs: Optional[int] = None,
        band: Optional[tuple[float, float]] = None,
        min_seconds: Optional[float] = None,
    ) -> dict[str, Any]:
        """The matrix as a dict, for a host that draws it its own way.

        Args:
            x: The column attribute; the first one not in *conditions* if
                ``None``.
            y: The row attribute; the next free one if ``None``; ``""`` for a
                single row.
            conditions: The cells clicked into, outermost first:
                ``[(attribute, bucket), ...]`` or a mapping.
            scenario: Only this scenario's runs.
            environment: Only this environment's runs (``None``: those that
                recorded none); every environment when left out (:data:`ANY`).
            min_runs, band: When a cell is judged rather than tentative
                (defaults: :data:`~.report.CELL_MIN_RUNS`,
                :data:`~.report.CELL_BAND`).
            min_seconds: The least a run must stay in a cell to count as in it.
        """
        q = self._query(x, y, conditions, scenario, environment, min_runs, band, min_seconds)
        out: dict[str, Any] = json.loads(self._core.matrix_json(q))
        return out

    def html(
        self,
        *,
        x: Optional[str] = None,
        y: Optional[str] = None,
        conditions: Optional[Conditions] = None,
        scenario: Optional[str] = None,
        environment: Any = ANY,
        min_runs: Optional[int] = None,
        band: Optional[tuple[float, float]] = None,
        min_seconds: Optional[float] = None,
        id: str = "",
        link: Optional[str] = None,
        labels: Optional[Mapping[str, str]] = None,
    ) -> str:
        """One matrix as an HTML fragment, host-driven: a click on a cell
        dispatches ``openodd:drill`` and, with *link* (``{conditions}`` is
        replaced by ``at=attribute%3Dbucket&...``), follows it. *id* is the
        root element's id; *labels* overrides the component's words by name
        (``title``, ``whole``, ``state_flaky``, ...). The arguments are
        :meth:`matrix`'s."""
        q = self._query(x, y, conditions, scenario, environment, min_runs, band, min_seconds)
        return str(self._core.fragment(q, id, link, dict(labels) if labels else None))

    def explorer_html(
        self,
        *,
        depth: int = 2,
        x: Optional[str] = None,
        y: Optional[str] = None,
        conditions: Optional[Conditions] = None,
        scenario: Optional[str] = None,
        environment: Any = ANY,
        min_runs: Optional[int] = None,
        band: Optional[tuple[float, float]] = None,
        min_seconds: Optional[float] = None,
        id: str = "",
        labels: Optional[Mapping[str, str]] = None,
    ) -> str:
        """The matrix and, *depth* levels down, the matrix inside each visited
        cell, rendered ahead of time as panels a click shows: self-contained,
        for a static report (needs :func:`script` on the page)."""
        if depth < 0:
            raise ValueError(f"depth must be 0 or more, not {depth}")
        q = self._query(x, y, conditions, scenario, environment, min_runs, band, min_seconds)
        return str(self._core.explorer(q, int(depth), id, dict(labels) if labels else None))

    def page(self, title: str = "ODD coverage", *, depth: int = 2, **query: Any) -> str:
        """A standalone page with the explorer (:meth:`explorer_html`)."""
        return page(title, self.explorer_html(depth=depth, **query))


class CasePlot:
    """The drawn cases of a sweep, as a scatter: every distinct case at the
    values it was drawn with, along any two attributes of its space.

    It reads the ``case`` block of coverage documents
    (:meth:`~.collector.CoverageCollector.record_case`) and their
    ``outcome``. Runs with the same overrides ran the same case and are one
    mark: a filled circle if they all passed, a cross if they all failed, a
    ring in the flaky colour (with ``passes/runs``) if they did both.
    Behind them, the plot is shaded by what the flaky model fitted to the
    runs predicts (:meth:`prediction`). Documents without a case or without
    an outcome are skipped and counted in a note.

    The component shares the coverage view's root class, styles and script:
    put :func:`styles` and :func:`script` on the page once for both.
    """

    def __init__(self, documents: Sequence[Mapping[str, Any]]) -> None:
        self._core = _core.CasePlot(json.dumps(list(documents)))

    @property
    def attributes(self) -> list[str]:
        """The attributes of the cases' space, in the order first seen."""
        return list(self._core.attributes)

    @property
    def scenarios(self) -> list[str]:
        return list(self._core.scenarios)

    @property
    def cases(self) -> int:
        """Distinct cases with a run that recorded an outcome."""
        return int(self._core.cases)

    @property
    def runs(self) -> int:
        """Runs placed: with a case and an outcome."""
        return int(self._core.runs)

    @property
    def skipped(self) -> tuple[int, int]:
        """Documents not placed: ``(without a case, without an outcome)``."""
        a, b = self._core.skipped
        return int(a), int(b)

    @staticmethod
    def _query(x: Optional[str], y: Optional[str], scenario: Optional[str], environment: Any) -> str:
        q: dict[str, Any] = {"x": x, "y": y, "scenario": scenario}
        if environment is not ANY:
            q["environment"] = environment
        return json.dumps(q)

    def data(
        self,
        *,
        x: Optional[str] = None,
        y: Optional[str] = None,
        scenario: Optional[str] = None,
        environment: Any = ANY,
        prediction: bool = True,
    ) -> dict[str, Any]:
        """The plot as a dict, for a host that draws it its own way: both
        axes (``kind``, ``domain``, ``labels``, the ODD ``buckets``) and every
        case (``values``, ``buckets``, ``runs``, ``passes``, ``verdict``,
        ``chosen_for``, ``flaky``, ...).

        With *prediction*, also ``prediction``: the grid the background is
        drawn from (:meth:`prediction`), and each case's
        ``predicted_pass_probability``.

        Args:
            x: The horizontal attribute; the first free one (numeric first)
                if ``None``.
            y: The vertical attribute; the next one if ``None``.
            scenario: Only this scenario's cases.
            environment: Only this environment's runs (``None``: those that
                recorded none); every environment when left out (:data:`ANY`).
            prediction: Whether to fit the flaky model and add its grid.
        """
        q = self._query(x, y, scenario, environment)
        out: dict[str, Any] = json.loads(self._core.data_json(q, bool(prediction)))
        return out

    def prediction(
        self,
        *,
        x: Optional[str] = None,
        y: Optional[str] = None,
        scenario: Optional[str] = None,
        environment: Any = ANY,
    ) -> dict[str, Any]:
        """What the flaky model fitted to the plot's runs predicts over *x*
        and *y*: the background's grid, as data.

        The model is :class:`~.flaky.FlakinessModel`'s Gaussian-process
        classifier with its defaults, fitted to every run with an outcome
        (seed ``"report"``, as the merged report assesses runs). The result
        holds ``x`` and ``y`` (``edges`` of a numeric axis's cells, or the
        ``labels`` of a categorical one), ``cells`` (rows along *y*, columns
        along *x*: ``pass_probability``, ``flip_probability``,
        ``instability_confidence`` and ``state``), ``held`` (each attribute
        not plotted and the value it is held at: the runs' median, or most
        common label) and ``runs``; or ``{"unavailable": why}`` when there
        are too few runs, or runs of one outcome only.

        The background's colour is each cell's ``pass_probability`` (red at
        0, yellow at 0.5, green at 1). ``state`` follows the report's rule --
        ``flaky`` where ``instability_confidence`` is above 0.5, else
        ``pass`` / ``fail`` by ``pass_probability`` -- and is data only.
        """
        out: dict[str, Any] = self.data(x=x, y=y, scenario=scenario, environment=environment)["prediction"]
        return out

    def html(
        self,
        *,
        x: Optional[str] = None,
        y: Optional[str] = None,
        scenario: Optional[str] = None,
        environment: Any = ANY,
        id: str = "",
        link: Optional[str] = None,
        table: bool = True,
        prediction: bool = True,
        instability_rings: bool = False,
        labels: Optional[Mapping[str, str]] = None,
    ) -> str:
        """One plot as an HTML fragment, host-driven: the axis pickers
        dispatch a cancelable ``openodd:axes`` event (``detail.view`` is
        ``"cases"``) and, with *link* (``{axes}`` is replaced by
        ``x=...&y=...``), go there. *table* adds the repeated cases' table
        below; *labels* overrides the component's words by name (``title``,
        ``state_flaky``, ...). The other arguments are :meth:`data`'s.

        *prediction* shades the plot by the pass probability the flaky
        model predicts (:meth:`prediction`), red low to yellow ~50% to green
        high, when the runs allow it -- runs of both outcomes,
        at least three -- and otherwise says why there is no background.
        *instability_rings* outlines the cases chosen for instability."""
        q = self._query(x, y, scenario, environment)
        return str(self._core.fragment(q, id, link, bool(table), bool(prediction),
                                       bool(instability_rings), dict(labels) if labels else None))

    def table_html(
        self,
        *,
        x: Optional[str] = None,
        y: Optional[str] = None,
        scenario: Optional[str] = None,
        environment: Any = ANY,
        id: str = "",
        labels: Optional[Mapping[str, str]] = None,
    ) -> str:
        """The cases run more than once, on their own: their values (*x* and
        *y* first), their outcomes in order and their verdict."""
        q = self._query(x, y, scenario, environment)
        return str(self._core.table(q, id, dict(labels) if labels else None))

    def explorer_html(
        self,
        *,
        x: Optional[str] = None,
        y: Optional[str] = None,
        scenario: Optional[str] = None,
        environment: Any = ANY,
        id: str = "",
        table: bool = True,
        prediction: bool = True,
        instability_rings: bool = False,
        labels: Optional[Mapping[str, str]] = None,
    ) -> str:
        """The plot of *x* and *y* and, as hidden panels the axis pickers
        show, every other pair of attributes (at most 100): self-contained,
        for a static report (needs :func:`script` on the page). The model
        behind *prediction* is fitted once for every pair."""
        q = self._query(x, y, scenario, environment)
        return str(self._core.explorer(q, id, bool(table), bool(prediction), bool(instability_rings),
                                       dict(labels) if labels else None))

    def page(self, title: str = "Drawn cases", **query: Any) -> str:
        """A standalone page with the explorer (:meth:`explorer_html`)."""
        return page(title, self.explorer_html(**query))
