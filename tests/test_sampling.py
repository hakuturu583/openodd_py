"""Sampling and coverage merging from Python."""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

import pytest

import openodd
from openodd import Backend, OddDraws, OddKnob, OddSample, OddSampler, load_and_merge, merge_coverage

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


def test_a_rule_valued_knobs_limits_do_not_narrow_what_is_drawn(tmp_path: Path) -> None:
    """#76: limits on the value written must not shrink the attribute's range."""
    (tmp_path / "taxonomy.yaml").write_text(RAIN_TAXONOMY)
    (tmp_path / "rules.yaml").write_text(PRECIPITATION_RULES.replace(
        "uniform: {low: [0, 20, 60], high: [5, 40, 90]}", "uniform: {low: [0, 0.2, 0.6], high: [0.05, 0.4, 0.9]}"
    ))
    rules = openodd.Rules.read(tmp_path / "rules.yaml")
    knob = OddKnob("env.wetness", quantity="carla_precipitation", limits=(0, 1), integer=False)
    b = Backend(probes={"rain_mm_h": (lambda c: c.get("rain"), "mm/h")}, rules=rules)
    o = openodd.load_openodd(
        RAIN_TAXONOMY,
        bindings={"weather.rainfall_rate": {"probe": "rain_mm_h", "buckets": [0, 2.5, 10]}},
        backend=b,
    )
    cases = OddSampler(o, knobs={"weather.rainfall_rate": knob}, seed=7, backend=b).sample(30)
    drawn = [c.values["weather.rainfall_rate"] for c in cases]
    assert max(drawn) > 2.5  # the upper bucket is reached, though the setting stays within 0..1
    assert {c.buckets["weather.rainfall_rate"] for c in cases} == {"[0, 2.5)", "[2.5, 10]"}
    for c in cases:
        (w,) = [x for x in c.overrides if x.startswith("env.wetness=")]
        assert 0 <= float(w.split("=")[1]) <= 1


def test_a_rule_valued_knob_reads_the_value_in_its_inputs_unit(tmp_path: Path) -> None:
    """#77: drawn in m/s, read by a rule written in km/h."""
    (tmp_path / "taxonomy.yaml").write_text("TAXONOMY:\n  ego:\n    speed: float velocity\n")
    (tmp_path / "rules.yaml").write_text(
        "version: 3\ntaxonomy: taxonomy.yaml\nbasis: {kind: expert}\n"
        "inputs:\n  speed: {concept: ego.speed, unit: km/h}\n"
        "quantities:\n  throttle:\n    parts:\n      p:\n"
        "        at: {speed: [0, 100]}\n        uniform: {low: [0, 100], high: [0, 100]}\n"
    )
    rules = openodd.Rules.read(tmp_path / "rules.yaml")
    b = Backend(probes={"v": (lambda c: c.get("v"), "m/s")}, rules=rules)
    o = openodd.load_openodd(
        "TAXONOMY:\n  ego:\n    speed: float velocity\n",
        bindings={"ego.speed": {"probe": "v", "buckets": [0, 10, 20]}},
        backend=b,
    )
    cases = OddSampler(o, knobs={"ego.speed": OddKnob("k", quantity="throttle")}, seed=3, backend=b).sample(10)
    for c in cases:
        (w,) = [x for x in c.overrides if x.startswith("k=")]
        # the rule gives its input back: km/h of the m/s drawn
        assert float(w.split("=")[1]) == pytest.approx(c.values["ego.speed"] * 3.6)


def test_the_case_keeps_what_the_rules_wrote_and_a_plot_places_it(tmp_path: Path) -> None:
    """#95: the quantities a knob wrote from the rules are recorded and plotted."""
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
    cases = OddSampler(o, knobs={"weather.rainfall_rate": knob}, seed=5, backend=b).sample(6)
    docs = []
    for case in cases:
        (written,) = [x for x in case.overrides if x.startswith("env.precipitation=")]
        assert case.quantities == {"carla_precipitation": float(written.split("=", 1)[1])}
        collector = openodd.CoverageCollector([], odd=o)
        collector.record_case(case)
        collector.record_outcome(True)
        doc = json.loads(json.dumps(collector.to_dict("rain")))
        assert doc["case"]["quantities"] == case.quantities
        docs.append(doc)
    # An integer knob's case keeps the value it ran with: the rounded one.
    whole = OddKnob("env.precipitation", quantity="carla_precipitation", limits=(0, 100), integer=True)
    for case in OddSampler(o, knobs={"weather.rainfall_rate": whole}, seed=5, backend=b).sample(6):
        (written,) = [x for x in case.overrides if x.startswith("env.precipitation=")]
        assert case.quantities["carla_precipitation"] == int(written.split("=", 1)[1])
    plot = openodd.CasePlot(docs)
    assert "carla_precipitation" in plot.attributes
    data = plot.data(x="weather.rainfall_rate", y="carla_precipitation")
    assert data["y"]["name"] == "carla_precipitation" and len(data["cases"]) == 6


# What a scenario sets up: a root of the network (the gap) and an effect of
# the rainfall rate drawn (how far a pedestrian is seen).
EFFECT_RULES = """
version: 3
taxonomy: taxonomy.yaml
basis: {kind: expert}
inputs:
  rainfall_rate: {concept: weather.rainfall_rate, unit: mm/h}
quantities:
  vehicle_ahead_gap:
    parts:
      staged:
        uniform: {low: 2, high: 30}
  pedestrian_seen_m:
    parts:
      rain:
        at: {rainfall_rate: [0, 10]}
        uniform: {low: [80, 20], high: [120, 40]}
"""


def _effects_setup(tmp_path: Path) -> tuple[Any, Any, Any]:
    (tmp_path / "taxonomy.yaml").write_text(RAIN_TAXONOMY)
    (tmp_path / "rules.yaml").write_text(EFFECT_RULES)
    rules = openodd.Rules.read(tmp_path / "rules.yaml")
    b = Backend(probes={"rain_mm_h": (lambda c: c.get("rain"), "mm/h")}, rules=rules)
    o = openodd.load_openodd(
        RAIN_TAXONOMY,
        bindings={"weather.rainfall_rate": {"probe": "rain_mm_h", "buckets": [0, 2.5, 10]}},
        backend=b,
    )
    return rules, b, o


def _written(case: OddSample, key: str) -> float:
    (written,) = [x for x in case.overrides if x.startswith(f"{key}=")]
    return float(written.split("=", 1)[1])


def test_effects_are_drawn_from_the_rules(tmp_path: Path) -> None:
    """A scenario's settings drawn from the network: a root, and an effect of
    the condition drawn, each at a quantile of its own."""
    rules, b, o = _effects_setup(tmp_path)
    effects = [
        OddKnob("scenario.npc_gap_m", quantity="vehicle_ahead_gap"),
        OddKnob("scenario.sight_m", quantity="pedestrian_seen_m", integer=True),
    ]
    rain = {"weather.rainfall_rate": OddKnob("env.rain_mm_h", range=(0, 10))}
    sampler = OddSampler(o, rain, seed=4, backend=b, effects=effects)
    assert sampler.effects == ["vehicle_ahead_gap", "pedestrian_seen_m"]
    cases = sampler.sample(30)
    gaps = []
    for case in cases:
        gap = _written(case, "scenario.npc_gap_m")
        assert 2 <= gap <= 30 and case.quantities["vehicle_ahead_gap"] == gap
        low, high = rules.range("pedestrian_seen_m", {"rainfall_rate": case.values["weather.rainfall_rate"]})
        sight = _written(case, "scenario.sight_m")
        assert sight == int(sight) and low - 0.5 <= sight <= high + 0.5
        gaps.append(gap)
    assert len(set(gaps)) > 20  # spread over the range, not one value
    again = OddSampler(o, rain, seed=4, backend=b, effects=effects).sample(30)
    assert [c.overrides for c in again] == [c.overrides for c in cases]


def test_a_control_no_attribute_reads_is_an_effect(tmp_path: Path) -> None:
    _, b, o = _effects_setup(tmp_path)
    controls = {
        "vehicle_ahead_gap_m": OddKnob("scenario.npc_gap_m", quantity="vehicle_ahead_gap", limits=(5, 25)),
        "unused_m": OddKnob("scenario.unused"),  # no quantity: nothing to draw
    }
    rain = {"weather.rainfall_rate": OddKnob("env.rain_mm_h", range=(0, 10))}
    cases = OddSampler(o, rain, controls=controls, seed=1, backend=b).sample(10)
    for case in cases:
        assert 5 <= _written(case, "scenario.npc_gap_m") <= 25
        assert not any(x.startswith("scenario.unused=") for x in case.overrides)


def test_an_effect_is_written_through_its_scale_and_offset(tmp_path: Path) -> None:
    """A quantity of a relative value written to a setting that takes the
    absolute one: offset by what it is relative to, then held to the limits;
    the case keeps the quantity the setting stands for."""
    _, b, o = _effects_setup(tmp_path)
    effects = [
        OddKnob("scenario.npc_gap_m", quantity="vehicle_ahead_gap", offset=100.0, limits=(0, 200)),
        OddKnob("scenario.half_gap", quantity="vehicle_ahead_gap", scale=0.5),
    ]
    rain = {"weather.rainfall_rate": OddKnob("env.rain_mm_h", range=(0, 10))}
    for case in OddSampler(o, rain, seed=2, backend=b, effects=effects).sample(20):
        gap = case.quantities["vehicle_ahead_gap"]
        assert 2 <= gap <= 30
        assert _written(case, "scenario.npc_gap_m") == pytest.approx(gap + 100.0)
        assert _written(case, "scenario.half_gap") == pytest.approx(gap * 0.5)
    # Held by the limits: what the case ran with is the quantity at the limit.
    held = [OddKnob("scenario.npc_gap_m", quantity="vehicle_ahead_gap", offset=100.0, limits=(0, 110))]
    for case in OddSampler(o, rain, seed=2, backend=b, effects=held).sample(20):
        written = _written(case, "scenario.npc_gap_m")
        assert written <= 110.0
        assert case.quantities["vehicle_ahead_gap"] == pytest.approx(written - 100.0)


def test_effects_need_rules_that_give_them(tmp_path: Path) -> None:
    rules, b, o = _effects_setup(tmp_path)
    with pytest.raises(ValueError, match="no quantity 'nope'"):
        OddSampler(o, backend=b, effects=[OddKnob("k", quantity="nope")])
    with pytest.raises(ValueError, match="writes no quantity"):
        OddSampler(o, backend=b, effects=[OddKnob("k")])
    bare = Backend(probes={"rain_mm_h": (lambda c: c.get("rain"), "mm/h")})
    with pytest.raises(ValueError, match="the backend has none"):
        OddSampler(o, backend=bare, effects=[OddKnob("k", quantity="vehicle_ahead_gap")])
    # An input standing for no attribute of the ODD, with no default.
    other = openodd.load_openodd("TAXONOMY:\n  speed: float velocity\n", backend=b)
    with pytest.raises(ValueError, match="read rainfall_rate, which stands for no attribute"):
        OddSampler(other, backend=b, effects=[OddKnob("k", quantity="pedestrian_seen_m")])


def test_an_effect_reads_a_quantity_at_the_value_written(tmp_path: Path) -> None:
    """A quantity a knob already wrote, from an attribute or an effect before,
    is read at that value -- after its limits and rounding."""
    (tmp_path / "taxonomy.yaml").write_text(RAIN_TAXONOMY)
    (tmp_path / "rules.yaml").write_text(
        PRECIPITATION_RULES
        + "  wet_copy:\n    parts:\n      same:\n        at: {carla_precipitation: [0, 100]}\n"
        "        uniform: {low: [0, 100], high: [0, 100]}\n"
        "  staged:\n    parts:\n      base:\n        uniform: {low: 0, high: 10}\n"
        "  staged_copy:\n    parts:\n      same:\n        at: {staged: [0, 10]}\n"
        "        uniform: {low: [0, 10], high: [0, 10]}\n"
    )
    rules = openodd.Rules.read(tmp_path / "rules.yaml")
    b = Backend(probes={"rain_mm_h": (lambda c: c.get("rain"), "mm/h")}, rules=rules)
    o = openodd.load_openodd(
        RAIN_TAXONOMY,
        bindings={"weather.rainfall_rate": {"probe": "rain_mm_h", "buckets": [0, 2.5, 10]}},
        backend=b,
    )
    knobs = {"weather.rainfall_rate": OddKnob("env.precipitation", quantity="carla_precipitation", integer=True)}
    effects = [
        OddKnob("scenario.wet", quantity="wet_copy"),
        OddKnob("scenario.staged", quantity="staged", integer=True),
        OddKnob("scenario.staged_copy", quantity="staged_copy"),
    ]
    for case in OddSampler(o, knobs, seed=6, backend=b, effects=effects).sample(20):
        assert _written(case, "scenario.wet") == pytest.approx(_written(case, "env.precipitation"))
        assert _written(case, "scenario.staged_copy") == pytest.approx(_written(case, "scenario.staged"))


def test_an_effect_input_the_sampler_does_not_draw_is_refused(tmp_path: Path) -> None:
    _, b, _ = _effects_setup(tmp_path)
    o = openodd.load_openodd(
        RAIN_TAXONOMY + "  ego:\n    speed: float velocity\n",
        bindings={
            "weather.rainfall_rate": {"probe": "rain_mm_h", "buckets": [0, 2.5, 10]},
            "ego.speed": {"probe": "rain_mm_h", "buckets": [0, 10]},
        },
        backend=b,
    )
    speed = {"ego.speed": OddKnob("ego.speed", range=(0, 10))}
    # The rainfall rate is an attribute, but no knob draws it.
    with pytest.raises(ValueError, match="read rainfall_rate, which stands for no attribute"):
        OddSampler(o, speed, backend=b, effects=[OddKnob("k", quantity="pedestrian_seen_m")])
    # A root reads nothing: it is drawn.
    OddSampler(o, speed, backend=b, effects=[OddKnob("k", quantity="vehicle_ahead_gap")]).sample(2)


def test_a_knob_on_an_attribute_reads_an_effect_at_the_value_written(tmp_path: Path) -> None:
    """Knobs on attributes and effects are realized together, in the rules'
    order: a quantity an attribute's knob writes reads an effect's at the
    value written to the run."""
    (tmp_path / "taxonomy.yaml").write_text(RAIN_TAXONOMY)
    (tmp_path / "rules.yaml").write_text(
        "version: 3\ntaxonomy: taxonomy.yaml\nbasis: {kind: expert}\n"
        "inputs:\n  rainfall_rate: {concept: weather.rainfall_rate, unit: mm/h}\n"
        "quantities:\n"
        "  staged:\n    parts:\n      base:\n        uniform: {low: 0, high: 10}\n"
        "  rain_and_staged:\n    parts:\n      same:\n"
        "        at: {rainfall_rate: [0, 10], staged: [0, 10]}\n"
        "        uniform: {low: [[0, 10], [0, 10]], high: [[0, 10], [0, 10]]}\n"
    )
    rules = openodd.Rules.read(tmp_path / "rules.yaml")
    b = Backend(probes={"rain_mm_h": (lambda c: c.get("rain"), "mm/h")}, rules=rules)
    o = openodd.load_openodd(
        RAIN_TAXONOMY,
        bindings={"weather.rainfall_rate": {"probe": "rain_mm_h", "buckets": [0, 2.5, 10]}},
        backend=b,
    )
    knobs = {"weather.rainfall_rate": OddKnob("env.copy", quantity="rain_and_staged")}
    effects = [OddKnob("scenario.staged", quantity="staged", integer=True)]
    for case in OddSampler(o, knobs, seed=8, backend=b, effects=effects).sample(20):
        # rain_and_staged copies staged, whatever the rain: as written, rounded.
        assert _written(case, "env.copy") == pytest.approx(_written(case, "scenario.staged"))


def test_effects_alone_are_drawn(tmp_path: Path) -> None:
    """With only effects, each case leaves the ODD's attributes open and
    writes the effects; with neither, there is nothing to draw."""
    _, b, o = _effects_setup(tmp_path)
    effects = [OddKnob("scenario.npc_gap_m", quantity="vehicle_ahead_gap")]
    cases = OddSampler(o, seed=1, backend=b, effects=effects).sample(5)
    assert [c.index for c in cases] == list(range(5))
    for case in cases:
        # No attribute drawn; the effect's quantile is the case's one parameter.
        assert list(case.values) == ["quantile(vehicle_ahead_gap)"]
        assert _written(case, "scenario.npc_gap_m") == pytest.approx(2 + 28 * case.values["quantile(vehicle_ahead_gap)"])
    with pytest.raises(ValueError, match="nothing of ODD .* can be drawn"):
        OddSampler(o, backend=b)


def test_a_control_an_explicit_knob_replaces_is_no_effect(tmp_path: Path) -> None:
    """A control the ODD's attribute reads stays the attribute's when an
    explicit knob replaces it: it is not drawn as an effect besides."""
    _, b, o = _effects_setup(tmp_path)
    b.readers["measure"] = lambda k: ((lambda c, k=k: c.get(k)), "mm/h")
    o = openodd.load_openodd(
        RAIN_TAXONOMY,
        bindings={"weather.rainfall_rate": {"measure": "rain_mm_h", "buckets": [0, 2.5, 10]}},
        backend=b,
    )
    controls = {"rain_mm_h": OddKnob("env.from_control", quantity="vehicle_ahead_gap")}
    explicit = {"weather.rainfall_rate": OddKnob("env.explicit", range=(0, 10))}
    sampler = OddSampler(o, explicit, controls=controls, seed=1, backend=b)
    assert sampler.effects == []
    for case in sampler.sample(5):
        assert not any(x.startswith("env.from_control=") for x in case.overrides)


def test_an_effect_reads_a_categorical_cause_at_its_label(tmp_path: Path) -> None:
    """A categorical attribute is a cause of the network: the effect is read
    at the label the case drew."""
    taxonomy = "TAXONOMY:\n  weather:\n    rain: [none, light, heavy]\n"
    (tmp_path / "taxonomy.yaml").write_text(taxonomy)
    (tmp_path / "rules.yaml").write_text(
        "version: 3\ntaxonomy: taxonomy.yaml\nbasis: {kind: expert}\n"
        "inputs:\n  rain: {concept: weather.rain}\n"
        "quantities:\n  sight_m:\n    parts:\n      p:\n"
        "        at: {rain: [none, light, heavy]}\n"
        "        uniform: {low: [300, 150, 50], high: [400, 250, 100]}\n"
    )
    rules = openodd.Rules.read(tmp_path / "rules.yaml")
    b = Backend(probes={"rain": (lambda c: c.get("rain"), "")}, rules=rules)
    o = openodd.load_openodd(taxonomy, bindings={"weather.rain": {"probe": "rain"}}, backend=b)
    rain = {"weather.rain": OddKnob("env.rain", values={"none": 0, "light": [1, 30], "heavy": [70, 100]})}
    effects = [OddKnob("scenario.sight_m", quantity="sight_m")]
    cases = OddSampler(o, rain, seed=2, backend=b, effects=effects).sample(30)
    seen = set()
    for case in cases:
        label = case.values["weather.rain"]
        seen.add(label)
        low, high = rules.range("sight_m", {"rain": label})
        assert low <= _written(case, "scenario.sight_m") <= high, (label, case.overrides)
    assert seen == {"none", "light", "heavy"}


def test_a_values_table_is_a_rule() -> None:
    """A categorical attribute's knob table is a rule: a uniform distribution
    at each label, reading the label drawn (issue #40)."""
    sampler = OddSampler(odd(), controls=CONTROLS, seed=5, backend=backend())
    rules = sampler.knob_rules
    assert rules is not None and rules.quantities == ["env_rain"]
    assert rules.basis("env_rain", "values")["kind"] == "expert"
    assert rules.range("env_rain", {"rain": "light"}) == (1.0, 30.0)
    seen = set()
    for case in sampler.sample(60):
        label = case.values["rain"]
        seen.add(label)
        low, high = {"none": (0, 0), "light": (1, 30), "heavy": (70, 100)}[label]
        written = _written(case, "env.rain")
        assert low <= written <= high, (label, case.overrides)
        assert case.quantities["env_rain"] == written
        # Written where the knob always was, at the case's own quantile: a
        # parameter of the case, like an effect's.
        assert [o.split("=")[0] for o in case.overrides] == ["env.rain", "scenario.gap"]
        assert set(case.values) == {"rain", "gap", "quantile(env_rain)"}
        at = case.values["quantile(env_rain)"]
        assert written == pytest.approx(low + at * (high - low))
    assert seen == {"none", "light", "heavy"}
    assert sampler.table_quantities == ["env_rain"]
    # The flaky search moves it: it is a dimension of the space.
    assert openodd.ParameterSpace.of(sampler).numeric["quantile(env_rain)"] == (0.0, 1.0)
    # The same seed writes the same values.
    first = OddSampler(odd(), controls=CONTROLS, seed=5, backend=backend()).sample(10)
    again = OddSampler(odd(), controls=CONTROLS, seed=5, backend=backend()).sample(10)
    assert [c.overrides for c in first] == [c.overrides for c in again]


def test_a_rule_of_the_tables_name_is_followed_instead(tmp_path: Path) -> None:
    """A quantity of the knob's name (``env.rain`` -> ``env_rain``) in the
    backend's rules -- the table measured -- writes the value instead."""
    taxonomy = "TAXONOMY:\n  weather:\n    rain: [none, light, heavy]\n"
    (tmp_path / "taxonomy.yaml").write_text(taxonomy)
    (tmp_path / "rules.yaml").write_text(
        "version: 3\ntaxonomy: taxonomy.yaml\nbasis: {kind: fitted}\n"
        "inputs:\n  rain: {concept: weather.rain}\n"
        "quantities:\n  env_rain:\n    parts:\n      p:\n"
        "        at: {rain: [none, light, heavy]}\n"
        "        uniform: {low: [0, 2, 5], high: [0, 3, 6]}\n"
    )
    rules = openodd.Rules.read(tmp_path / "rules.yaml")
    b = Backend(probes={"rain": (lambda c: c.get("rain"), "")}, rules=rules)
    o = openodd.load_openodd(taxonomy, bindings={"weather.rain": {"probe": "rain"}}, backend=b)
    knob = OddKnob("env.rain", values={"none": 0, "light": [1, 30], "heavy": [70, 100]})
    sampler = OddSampler(o, {"weather.rain": knob}, seed=2, backend=b)
    assert sampler.knob_rules is None
    for case in sampler.sample(30):
        low, high = rules.range("env_rain", {"rain": case.values["weather.rain"]})
        assert low <= _written(case, "env.rain") <= high


def test_other_tables_are_not_rules() -> None:
    """A numeric attribute's table (ranges replacing the interval drawn) and a
    table of labels write what they always did."""
    numeric = OddSampler(
        odd(),
        {"rain": OddKnob("env.rain", values={"none": "dry", "light": "wet", "heavy": "wet"}),
         "gap": OddKnob("scenario.gap", values={"near": [2, 9], "far": [20, 35]})},
        seed=1,
        backend=Backend(),
    )
    assert numeric.knob_rules is None
    for case in numeric.sample(20):
        assert not case.quantities
        assert any(o in ("env.rain=dry", "env.rain=wet") for o in case.overrides)


def test_table_rule_names() -> None:
    from openodd.sampler import table_rule_name

    assert table_rule_name("environment.precipitation") == "environment_precipitation"
    assert table_rule_name("a-b c") == "a_b_c"
    assert table_rule_name("3d") == "_3d"


def test_a_table_whose_key_is_the_attributes_name() -> None:
    """An attribute ``rain`` with a knob ``rain``: the input the rule reads
    is kept apart from the quantity it writes."""
    o = openodd.load_openodd(ODD, bindings={"rain": {"probe": "rain"}}, backend=backend())
    knob = OddKnob("rain", values={"none": 0, "light": [1, 30], "heavy": 90})
    sampler = OddSampler(o, {"rain": knob}, seed=1, backend=backend())
    assert sampler.knob_rules.quantities == ["rain"]
    assert sampler.knob_rules.range("rain", {"rain_label": "heavy"}) == (90.0, 90.0)
    for case in sampler.sample(10):
        if case.values["rain"] == "heavy":
            assert _written(case, "rain") == 90


def test_an_integer_table_draws_among_the_integers_of_its_range() -> None:
    """``integer`` keeps the table's ranges half-open: ``[1, 3)`` writes 1 or
    2, each as likely, never 3."""
    knob = OddKnob("env.rain", values={"none": 0, "light": [1, 3], "heavy": [70, 71]}, integer=True)
    sampler = OddSampler(odd(), {"rain": knob}, controls=CONTROLS, seed=2, backend=backend())
    written: dict[str, list[int]] = {"none": [], "light": [], "heavy": []}
    for case in sampler.sample(200):
        (w,) = [o for o in case.overrides if o.startswith("env.rain=")]
        assert w.split("=")[1].isdigit()
        written[case.values["rain"]].append(int(w.split("=")[1]))
    assert set(written["none"]) == {0}
    assert set(written["light"]) == {1, 2}
    assert 0.3 < written["light"].count(1) / len(written["light"]) < 0.7
    assert set(written["heavy"]) == {70}


def test_keys_that_read_the_same_keep_their_own_quantities() -> None:
    """``env.rain`` and ``env_rain`` are one rule name: the second table
    takes ``env_rain_2``, and each writes its own setting."""
    odd2 = openodd.load_openodd(
        "TAXONOMY:\n  rain: [none, heavy]\n  fog: [none, heavy]\n",
        bindings={"rain": {"probe": "rain"}, "fog": {"probe": "rain"}},
        backend=backend(),
    )
    knobs = {
        "rain": OddKnob("env.rain", values={"none": 0, "heavy": 90}),
        "fog": OddKnob("env_rain", values={"none": 5, "heavy": 50}),
    }
    sampler = OddSampler(odd2, knobs, seed=1, backend=backend())
    assert sampler.table_quantities == ["env_rain", "env_rain_2"]
    for case in sampler.sample(10):
        assert _written(case, "env.rain") == {"none": 0, "heavy": 90}[case.values["rain"]]
        assert _written(case, "env_rain") == {"none": 5, "heavy": 50}[case.values["fog"]]
