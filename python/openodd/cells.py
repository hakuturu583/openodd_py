"""Cells: how long a run spent in each combination of the ODD's buckets.

Every step, each attribute with buckets puts its value in one (its cover
item's ``bucket_of``); the labels together are the step's *cell*.
:class:`OddCells` adds up steps, seconds and metres per cell, so a consumer can
slice a run by any attributes afterwards -- a matrix of two of them inside a
third's bucket -- without keeping every step's values, and without bucketing
them again by rules of its own::

    cells = OddCells(odd)
    for step in run:                       # every attribute's value, in order
        cells.add(step.values, seconds=step.dt, meters=step.ds)
    cells.to_dict()
    # {"attributes": ["scenery.speed_limit", "environment.rain"],
    #  "rows": [[["[50, 60)", "none"], 812, 40.6, 588.2], ...]}

A missing value is :data:`MISSING`; a number outside the buckets
``<below>``/``<above>``; a category none of the values names keeps its own
label. Attributes without buckets are left out of the cell.
"""

from __future__ import annotations

import json
from collections.abc import Mapping, Sequence
from typing import TYPE_CHECKING, Any, Union

from . import _core
from .model import to_core

if TYPE_CHECKING:
    from .model import OddDefinition

__all__ = ["MISSING", "OddCells"]

#: The label of a step whose value was missing.
MISSING: str = _core.MISSING


class OddCells:
    """Steps, seconds and metres per cell of *odd*'s attributes."""

    def __init__(self, odd: "OddDefinition") -> None:
        self._names = [a.name for a in odd.attributes]
        self._core = _core.Cells(odd._core)

    @property
    def attributes(self) -> list[str]:
        """The attributes with buckets: what a cell's labels are of, in order."""
        return list(self._core.attributes)

    @property
    def ticks(self) -> int:
        """Steps counted so far."""
        return int(self._core.ticks)

    def _values(self, values: Union[Sequence[Any], Mapping[str, Any]]) -> list[Any]:
        if isinstance(values, Mapping):
            return [to_core(values.get(name)) for name in self._names]
        return [to_core(v) for v in values]

    def cell_of(self, values: Union[Sequence[Any], Mapping[str, Any]]) -> list[str]:
        """The cell of one step: *values* by attribute name, or one per
        attribute of the ODD in its order."""
        return list(self._core.cell_of(self._values(values)))

    def add(self, values: Union[Sequence[Any], Mapping[str, Any]], seconds: float = 0.0,
            meters: float = 0.0) -> list[str]:
        """Count one step that took *seconds* and covered *meters*; the cell it
        was counted in."""
        return list(self._core.add(self._values(values), float(seconds), float(meters)))

    def to_dict(self) -> dict[str, Any]:
        """``{"attributes": [...], "rows": [[[label, ...], ticks, seconds, meters], ...]}``,
        most steps first (ties by labels); seconds and metres to 3 decimals."""
        out: dict[str, Any] = json.loads(self._core.to_json())
        return out
