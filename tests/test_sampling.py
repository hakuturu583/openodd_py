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


ODD_WITH_REGIONS = """
TAXONOMY:
  rain: [none, light, heavy]
  gap: float length
  lead: float length
ODD:
  o:
    EXCLUDE_AND:
      rain: heavy
      gap: "< 10 m"
MODULES:
  wet:
    INCLUDE_AND:
      rain: heavy
  close:
    INCLUDE_AND:
      gap: "< 20 m"
  hidden:
    INCLUDE_AND:
      lead: "> 10 m"
"""


def regions() -> openodd.OddDefinition:
    return openodd.load_openodd(
        ODD_WITH_REGIONS,
        bindings={
            "rain": {"probe": "rain"},
            "gap": {"measure": "gap_m", "buckets": [0, 10, 20, 40]},
            "lead": {"measure": "lead_m"},
        },
        name="o",
        backend=backend(),
    )


def test_weighted_holds_each_module_to_its_share() -> None:
    o = regions()
    sampler = OddSampler(o, controls=CONTROLS, seed=3, weights={"wet": 0.3, "close": 0.6}, backend=backend())
    assert sampler.strategy == "weighted"
    assert sampler.weights == {"wet": 0.3, "close": 0.6}
    cases = sampler.sample(100)
    assert [c.index for c in cases] == list(range(100))
    assert sum(c.buckets["rain"] == "heavy" for c in cases) == 30
    assert sum(c.values["gap"] < 20 for c in cases) == pytest.approx(60, abs=1)
    for case in cases:
        assert not (case.buckets["rain"] == "heavy" and case.values["gap"] < 10)


def test_weighted_batches_go_on_from_each_other() -> None:
    o = regions()
    draws = OddDraws()
    for _ in range(10):
        sampler = OddSampler(
            o, controls=CONTROLS, seed=4, weights={"wet": 0.2}, drawn=draws.of(o), backend=backend()
        )
        draws.add(o, sampler.sample(2))
    assert sum(c.buckets["rain"] == "heavy" for c in draws.of(o)) == 4


def test_weighted_refuses_what_it_cannot_hold() -> None:
    o = regions()
    with pytest.raises(ValueError, match="no module dry"):
        OddSampler(o, controls=CONTROLS, weights={"dry": 0.5}, backend=backend())
    with pytest.raises(ValueError, match="root"):
        OddSampler(o, controls=CONTROLS, weights={"o": 0.5}, backend=backend())
    with pytest.raises(ValueError, match="'weighted'"):
        OddSampler(o, controls=CONTROLS, strategy="coverage", weights={"wet": 0.5}, backend=backend())
    with pytest.raises(ValueError, match="needs a share"):
        OddSampler(o, controls=CONTROLS, strategy="weighted", backend=backend())
    # Nothing draws the lead, so whether a case is in `hidden` is not known.
    sampler = OddSampler(o, controls=CONTROLS, weights={"hidden": 0.5}, backend=backend())
    with pytest.raises(ValueError, match="does not follow from what the knobs set"):
        sampler.sample(4)


RAIN_TAXONOMY = """
TAXONOMY:
  weather:
    rainfall_rate: float precipitation_rate
"""

# CARLA's precipitation as an effect of the rainfall rate drawn.
PRECIPITATION_RULES = """
version: 3
taxonomy: taxonomy.yaml
basis: {kind: expert}
inputs:
  rainfall_rate: {concept: weather.rainfall_rate, unit: mm/h}
quantities:
  carla_precipitation:
    parts:
      rain:
        at: {rainfall_rate: [0, 2.5, 10]}
        uniform: {low: [0, 20, 60], high: [5, 40, 90]}
"""


def test_a_knob_writes_what_the_rules_make_of_the_value_drawn(tmp_path: Path) -> None:
    (tmp_path / "taxonomy.yaml").write_text(RAIN_TAXONOMY)
    (tmp_path / "rules.yaml").write_text(PRECIPITATION_RULES)
    rules = openodd.Rules.read(tmp_path / "rules.yaml")
    knob = OddKnob("env.precipitation", quantity="carla_precipitation", limits=(0, 100))
    b = Backend(probes={"rain_mm_h": (lambda c: c.get("rain"), "mm/h")}, rules=rules)
    o = openodd.load_openodd(
        RAIN_TAXONOMY,
        bindings={"weather.rainfall_rate": {"probe": "rain_mm_h", "buckets": [0, 2.5, 10]}},
        backend=b,
    )
    sampler = OddSampler(o, knobs={"weather.rainfall_rate": knob}, seed=5, backend=b)
    cases = sampler.sample(40)
    for case in cases:
        x = case.values["weather.rainfall_rate"]
        (written,) = [o for o in case.overrides if o.startswith("env.precipitation=")]
        y = float(written.split("=", 1)[1])
        low, high = rules.range("carla_precipitation", {"rainfall_rate": x})
        assert low <= y <= high, (x, y)
    # Reproducible from the seed, and not every case at one quantile.
    again = OddSampler(o, knobs={"weather.rainfall_rate": knob}, seed=5, backend=b).sample(40)
    assert [c.overrides for c in again] == [c.overrides for c in cases]
    quantiles = {
        round(
            (float(c.overrides[-1].split("=")[1]) - rules.range("carla_precipitation", {"rainfall_rate": c.values["weather.rainfall_rate"]})[0]),
            6,
        )
        for c in cases
    }
    assert len(quantiles) > 10


def test_a_knob_from_the_rules_needs_an_input_for_its_attribute() -> None:
    b = Backend(probes={"rain_mm_h": (lambda c: c.get("rain"), "mm/h")}, rules=openodd.Rules.from_text(
        "version: 2\nquantities:\n  p:\n    parts:\n      x:\n        at: {water: [0, 1]}\n        uniform: {low: 0, high: 1}\n"
    ))
    o = openodd.load_openodd(
        RAIN_TAXONOMY,
        bindings={"weather.rainfall_rate": {"probe": "rain_mm_h", "buckets": [0, 2.5, 10]}},
        backend=b,
    )
    with pytest.raises(ValueError, match="no input of the rules stands for weather.rainfall_rate"):
        OddSampler(o, knobs={"weather.rainfall_rate": OddKnob("k", quantity="p")}, backend=b)
    with pytest.raises(ValueError, match="no quantity 'nope'"):
        OddSampler(o, knobs={"weather.rainfall_rate": OddKnob("k", quantity="nope")}, backend=b)
