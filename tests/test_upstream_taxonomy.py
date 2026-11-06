"""The reader against tier4/taxonomy's export (#44).

A synthetic file in the export's shape runs always. The real file runs when
``OPENODD_TIER4_TAXONOMY`` names it: tier4/taxonomy is private, so it is not
kept here.
"""

from __future__ import annotations

import os
import re
from pathlib import Path

import pytest

import openodd

DATA = Path(__file__).parent / "data"
UPSTREAM = os.environ.get("OPENODD_TIER4_TAXONOMY")

# Defects of the upstream file itself, each reported there: until it is fixed,
# its message is an expected failure, not this reader's.
KNOWN_UPSTREAM_DEFECTS = {
    r"unknown concept 'visibility_range'": "tier4/taxonomy#46",
    r"unknown concept 'horizontal_plane_detail'": "tier4/taxonomy#55",
    r"unknown concept 'curvature'": "tier4/taxonomy#56",
}


def probes() -> openodd.Backend:
    return openodd.Backend(
        name="test",
        probes={
            "drizzle": (lambda c: c.get("drizzle"), "mm/h"),
            "gust": (lambda c: c.get("gust"), "m/s"),
            "flow": (lambda c: c.get("flow"), "veh/min"),
            "curvature": (lambda c: c.get("curvature"), ""),
        },
    )


def test_the_export_shape_reads() -> None:
    odd = openodd.load_openodd(
        DATA / "upstream_shaped_taxonomy.yaml",
        bindings={
            "environment.environment.weather.drizzle.drizzle_rate": {"probe": "drizzle"},
            "environment.environment.weather.gust.gust_speed": {"probe": "gust"},
            "environment.environment.traffic.flow_rate": {"probe": "flow"},
            "scenery.scenery.geometry.bend.bend_curvature": {"probe": "curvature"},
        },
        backend=probes(),
    )
    names = {a.name for a in odd.attributes}
    assert "scenery.scenery.notes.label" not in names
    sample = odd.sample({"drizzle": 1.0, "gust": 11.0, "flow": 3, "curvature": -0.02})
    weather = "environment.environment.weather"
    assert sample[f"{weather}.drizzle.drizzle_intensity"] == "much"  # 1.0 is not in [0.1 .. 1.0)
    assert sample[f"{weather}.gust.gust_intensity"] == "gusty"  # 11 m/s is 39.6 km/h
    assert sample["environment.environment.traffic.flow_level"] == "thin"  # 180 veh/h
    assert sample["scenery.scenery.geometry.bend.bend_detail"] == "to_left"
    assert sample["scenery.scenery.geometry.bend.bend_kind"] == "bending"


@pytest.mark.skipif(not UPSTREAM, reason="OPENODD_TIER4_TAXONOMY is not set")
def test_the_upstream_taxonomy_reads() -> None:
    try:
        odd = openodd.load_openodd(Path(UPSTREAM))
    except openodd.OpenOddError as e:
        for pattern, issue in KNOWN_UPSTREAM_DEFECTS.items():
            if re.search(pattern, str(e)):
                pytest.xfail(f"{issue}: {e}")
        raise
    names = [a.name for a in odd.attributes]
    levels = [n for n in names if n.endswith(("_intensity", "_level"))]
    assert levels, "no *_intensity or *_level concept read"
    # Each is one categorical, not a record whose literals became attributes.
    misread = [n for n in names if re.search(r"_(intensity|level)\.", n)]
    assert not misread, misread
