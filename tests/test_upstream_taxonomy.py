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


DEFAULT = Path(openodd.__file__).parent / "odds" / "default"
# Where the bundled default ODD departs from upstream on purpose: upstream's
# fog levels are told by a concept it does not declare (tier4/taxonomy#46).
DEFAULT_EXCEPTIONS = {
    "environment_conditions.environment_conditions.weather.fog.fog_level": (
        "visibility_range", "fog_visibility_range",
    ),
}


def _taxonomy(path: Path) -> dict:
    import yaml

    return yaml.safe_load(path.read_text())["TAXONOMY"]


def _leaves(tree: dict, prefix: str = "") -> dict[str, object]:
    """Each concept and what declares it: a primitive's type, a categorical's
    literals (with their definitions); records are walked into. A mapping is
    a categorical when each of its entries defines a literal by concepts
    beside it (`rain_intensity: {no_rain: {rainfall_rate: ...}}`)."""
    out: dict[str, object] = {}
    for key, value in tree.items():
        name = f"{prefix}{key}"
        categorical = isinstance(value, dict) and all(
            isinstance(v, dict) and v and set(v) <= set(tree) for v in value.values()
        )
        if isinstance(value, dict) and not categorical:
            out.update(_leaves(value, f"{name}."))
        else:
            out[name] = value
    return out


def _at(tree: dict, name: str) -> object:
    node: object = tree
    for key in name.split("."):
        if not isinstance(node, dict) or key not in node:
            return None
        node = node[key]
    return node


def _upstream_tree() -> dict:
    return _taxonomy(Path(UPSTREAM))


@pytest.mark.skipif(not UPSTREAM, reason="OPENODD_TIER4_TAXONOMY is not set")
def test_the_default_odd_is_a_subset_of_upstream() -> None:
    upstream = _upstream_tree()
    ours = _leaves(_taxonomy(DEFAULT / "default.yaml"))
    assert ours
    for name, declared in ours.items():
        theirs = _at(upstream, name)
        assert theirs is not None, f"{name}: upstream does not declare it"
        if name in DEFAULT_EXCEPTIONS:
            old, new = DEFAULT_EXCEPTIONS[name]
            theirs = {lit: {(new if k == old else k): v for k, v in (d or {}).items()}
                      for lit, d in theirs.items()}
        assert declared == theirs, f"{name}: {declared!r} here, {theirs!r} upstream"


@pytest.mark.skipif(not UPSTREAM, reason="OPENODD_TIER4_TAXONOMY is not set")
def test_what_is_pending_upstream_is_not_there_yet() -> None:
    upstream = _upstream_tree()
    for name in _leaves(_taxonomy(DEFAULT / "pending_upstream.yaml")):
        # Once upstream has it, it moves into default.yaml under upstream's id.
        assert _at(upstream, name) is None, f"{name}: upstream declares it now; move it"


def test_the_default_odd_and_what_is_pending_do_not_overlap() -> None:
    ours = _leaves(_taxonomy(DEFAULT / "default.yaml"))
    pending = _leaves(_taxonomy(DEFAULT / "pending_upstream.yaml"))
    assert ours and pending and not set(ours) & set(pending)
    tops = {n.split(".")[0] for n in ours}
    assert not tops & {n.split(".")[0] for n in pending}
