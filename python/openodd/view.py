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
"""

from __future__ import annotations

import json
from collections.abc import Iterable, Mapping, Sequence
from typing import Any, Optional, Union

from . import _core

__all__ = ["CoverageView", "page", "parse_conditions", "script", "styles"]

Conditions = Union[Mapping[str, str], Iterable[Sequence[str]]]

#: Leave ``environment`` out of a query: every environment.
ANY = object()


def styles() -> str:
    """The component's CSS, scoped under ``.oo-cov``; once per page."""
    return str(_core.coverage_view_styles())


def script() -> str:
    """The component's script (drill events, explorer panels); once per page."""
    return str(_core.coverage_view_script())


def page(title: str, body: str) -> str:
    """A standalone HTML page around *body*, with the styles and the script."""
    return str(_core.coverage_view_page(title, body))


def parse_conditions(values: Iterable[str]) -> list[tuple[str, str]]:
    """``attribute=bucket`` query values (the ``at`` of a cell's link), in
    order; one per attribute."""
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
                recorded none); every environment when left out.
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
