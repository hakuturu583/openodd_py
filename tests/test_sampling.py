"""Sampling and coverage merging from Python."""

from __future__ import annotations

import json
from pathlib import Path

import pytest

import openodd
from openodd import Backend, OddDraws, OddKnob, OddSampler, load_and_merge, merge_coverage

ODD = """
TAXONOMY:
  rain: [none, light, heavy]
  gap: float length
MODULES:
  bound:
    EXCLUDE_AND:
      rain: heavy
      gap: "< 10 m"
  wet:
    INCLUDE_AND:
      rain: heavy
"""

WEATHER = OddKnob("env.rain", values={"none": 0, "light": [1, 30], "heavy": [70, 100]})


def rain(context):  # the backend's probe, with a knob on it
    return context.get("rain")


def backend() -> Backend:
    return Backend(
        probes={"rain": (rain, "")},
        readers={"measure": lambda key: ((lambda c: c.get(key)), "m")},
        knobs={rain: WEATHER},
    )


def odd() -> openodd.OddDefinition:
    return openodd.load_openodd(
        ODD,
        bindings={"rain": {"probe": "rain"}, "gap": {"measure": "gap_m", "buckets": [0, 10, 20, 40]}},
        situations={"wet": {}},
        name="o",
        backend=backend(),
    )


CONTROLS = {"gap_m": OddKnob("scenario.gap", range=(2, 35))}


def test_cases_are_inside_the_odd_and_set_by_their_knobs() -> None:
    o = odd()
    sampler = OddSampler(o, controls=CONTROLS, seed=1, backend=backend())
    assert sampler.attributes == ["rain", "gap"]
    for case in sampler.sample(100):
        assert not (case.buckets["rain"] == "heavy" and case.values["gap"] < 10)
        keys = [o.split("=")[0] for o in case.overrides]
        assert keys == ["env.rain", "scenario.gap"]
        assert 2 <= case.values["gap"] <= 35


def test_coverage_aims_at_holes_and_batches_go_on() -> None:
    o = odd()
    draws = OddDraws()
    first = OddSampler(o, controls=CONTROLS, seed=2, strategy="coverage", backend=backend())
    assert first.situation_holes == ["wet"]
    cases = first.sample(2)
    assert cases[0].situation == "wet" and cases[0].buckets["rain"] == "heavy"
    draws.add(o, cases)
    nxt = OddSampler(o, controls=CONTROLS, seed=2, strategy="coverage", drawn=draws.of(o), backend=backend())
    assert nxt.situation_holes == []


def test_a_knob_must_reach_a_bucket() -> None:
    with pytest.raises(ValueError, match="no bucket of gap"):
        OddSampler(odd(), knobs={"gap": OddKnob("g", range=(100, 200))}, backend=backend())


def _coverage(tmp_path: Path, hit: str) -> Path:
    doc = {
        "schema": openodd.COVERAGE_SCHEMA,
        "scenario": "s",
        "items": [{
            "name": "odd.rain", "group": "odd", "kind": "categorical", "event": "tick",
            "buckets": ["none", "light", "heavy"], "target": 1, "outside_odd": [],
            "hits": {b: (5 if b == hit else 0) for b in ["none", "light", "heavy"]},
            "seconds": {"none": 0.0, "light": 0.0, "heavy": 0.0}, "samples": 5,
        }],
        "odd": {"name": "o", "ticks": {"inside": 5, "assumed": 0, "outside": 0},
                "seconds": {"inside": 0.25, "assumed": 0.0, "outside": 0.0}, "revision": "abc"},
    }
    path = tmp_path / f"{hit}_coverage.json"
    path.write_text(json.dumps(doc))
    return path


def test_merging_and_drawing_from_coverage(tmp_path: Path) -> None:
    _coverage(tmp_path, "none")
    _coverage(tmp_path, "light")
    report = load_and_merge([tmp_path])
    entry = report.entry("odd.rain")
    assert report.runs == 2 and entry.holes == ["heavy"] and entry.runs == {"none": 1, "light": 1}
    assert report.odds["o"].revisions == ["abc"]
    assert "| odd.rain | tick | 67% | 2/3 | heavy |" in report.to_markdown()
    sampler = OddSampler(odd(), controls=CONTROLS, seed=4, strategy="coverage", coverage=report, backend=backend())
    assert all(c.buckets["rain"] == "heavy" for c in sampler.sample(3)[:1])


def test_merge_refuses_other_documents() -> None:
    with pytest.raises(ValueError, match="not a coverage file"):
        merge_coverage([{"schema": "x"}])
