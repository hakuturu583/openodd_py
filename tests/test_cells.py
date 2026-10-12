"""Cells: steps per combination of an ODD's buckets, bucketed as the items are."""

from __future__ import annotations

import enum

import pytest

import openodd
from openodd import (
    ABOVE_RANGE,
    BELOW_RANGE,
    MISSING,
    CoverItem,
    OddAttribute,
    OddCells,
    OddDefinition,
    OddModule,
    SamplingEvent,
)


class Rain(enum.Enum):
    NONE = "none"
    HEAVY = "heavy"


def _odd() -> OddDefinition:
    speed = OddAttribute("speed", lambda c: c["speed"], unit="km/h", buckets=[0, 30, 60],
                         cover_by="seconds", min_stay=2)
    free = OddAttribute("free", lambda c: c["free"])  # no buckets: not in a cell
    rain = OddAttribute("rain", lambda c: c["rain"], values=list(Rain))
    root = OddModule("root", include_and=[speed.less_than(60)])
    return OddDefinition("cells", [speed, free, rain], [root])


def test_a_cell_is_the_items_buckets():
    odd = _odd()
    cells = OddCells(odd)
    assert cells.attributes == ["speed", "rain"]
    speed, rain = (a.item for a in odd.attributes if a.item is not None)
    for value in (0, 29.9, 30, 60):
        assert cells.cell_of([value, 1, Rain.NONE])[0] == speed.bucket_of(value)
    assert cells.cell_of([-1, None, Rain.HEAVY]) == [BELOW_RANGE, rain.bucket_of(Rain.HEAVY)]
    assert cells.cell_of([61, None, None]) == [ABOVE_RANGE, MISSING]
    assert cells.cell_of([None, None, "fog"]) == [MISSING, "fog"]
    assert cells.cell_of({"speed": 10, "rain": Rain.NONE}) == ["[0, 30)", "NONE"]
    with pytest.raises(ValueError):
        cells.cell_of([1, 2])


def test_cells_count_steps_seconds_and_metres():
    cells = OddCells(_odd())
    for _ in range(2):
        cells.add([40, None, Rain.HEAVY], seconds=0.0333333, meters=0.5)
    cells.add([10, None, Rain.NONE], seconds=0.05, meters=0.25)
    cells.add([10, None, Rain.HEAVY], seconds=0.05)
    assert cells.ticks == 4
    assert cells.to_dict() == {
        "attributes": ["speed", "rain"],
        "rows": [
            [["[30, 60]", "HEAVY"], 2, 0.067, 1.0],
            [["[0, 30)", "HEAVY"], 1, 0.05, 0.0],  # ties by labels
            [["[0, 30)", "NONE"], 1, 0.05, 0.25],
        ],
    }


def test_items_say_their_attribute_and_amount():
    odd = _odd()
    speed = next(a.item for a in odd.attributes if a.name == "speed")
    described = speed.describe()
    assert described["attribute"] == "speed" and described["amount"] == "counted.seconds"
    rain = next(a.item for a in odd.attributes if a.name == "rain").describe()
    assert rain["attribute"] == "rain" and rain["amount"] == "hits"
    plain = CoverItem("gap", lambda c: c, buckets=[0, 1], event=SamplingEvent.TICK,
                      cover_by="meters")
    assert plain.describe()["amount"] == "meters" and "attribute" not in plain.describe()
    assert [i.describe()["attribute"] for i in odd.cover_items()] == ["speed", "rain"]


def test_the_version_is_the_distributions():
    assert openodd.__version__ and openodd.__version__[0].isdigit()
