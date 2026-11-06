"""Rules as conditional distributions (src/rules.rs, openodd.rules) and the
backend that holds them."""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
from statistics import NormalDist

import pytest

from openodd import Backend, Rules, RulesError, UnclassifiedInputsWarning

GRIP = """
version: 2
quantities:
  grip:
    parts:
      water:
        at: {water: [0, 50, 100]}
        uniform:
          low: [0.90, 0.65, 0.45]
          high: [1.10, 0.85, 0.65]
"""


class TestRules:
    def test_a_range_is_interpolated_between_the_points(self) -> None:
        rules = Rules.from_text(GRIP)
        assert rules.quantities == ["grip"]
        assert rules.inputs("grip") == ["water"]
        assert rules.combine("grip") == "min"
        assert rules.range("grip", {"water": 25}) == pytest.approx((0.775, 0.975))
        assert rules.range("grip", {"water": 150}) == pytest.approx((0.45, 0.65))
        assert rules.evaluate("grip", {"water": 70}) == pytest.approx(
            {"low": 0.57, "high": 0.77, "median": 0.67, "mean": 0.67}
        )

    def test_a_run_draws_a_quantile(self) -> None:
        rules = Rules.from_text(GRIP)
        assert rules.quantile("grip", {"water": 70}, 0.25) == pytest.approx(0.62)
        assert rules.quantile("grip", {"water": 70}, 2.0) == pytest.approx(0.77)
        assert rules.cdf("grip", {"water": 70}, 0.62) == pytest.approx(0.25)
        assert rules.distributions("grip", {"water": 70}) == [
            ("water", "uniform", pytest.approx({"low": 0.57, "high": 0.77}))
        ]

    def test_a_truncated_normal_draws_as_the_standard_library_computes(self) -> None:
        text = GRIP.replace(
            "uniform:\n          low: [0.90, 0.65, 0.45]\n          high: [1.10, 0.85, 0.65]",
            "normal: {mean: [1.0, 0.75, 0.55], sd: 0.05, low: [0.9, 0.65, 0.45], high: [1.1, 0.85, 0.65]}",
        )
        rules = Rules.from_text(text)
        n = NormalDist(0.75, 0.05)
        lo, hi = n.cdf(0.65), n.cdf(0.85)
        for p in (0.01, 0.25, 0.5, 0.9):
            want = n.inv_cdf(lo + p * (hi - lo))
            assert rules.quantile("grip", {"water": 50}, p) == pytest.approx(want, abs=1e-9)

    def test_parts_move_together_and_the_least_holds(self) -> None:
        text = """
version: 2
quantities:
  visibility:
    combine: min
    parts:
      fog:
        at: {fog_density: [0, 100]}
        uniform: {low: [250, 15], high: [400, 40]}
      light:
        at: {sun_altitude_angle: [-6, 0]}
        uniform: {low: [50, 250], high: [150, 400]}
"""
        rules = Rules.from_text(text)
        night = {"fog_density": 70, "sun_altitude_angle": -20}
        assert rules.range("visibility", night) == pytest.approx((50, 148))
        assert rules.quantile("visibility", night, 0.5) == pytest.approx(100)
        assert rules.quantile("visibility", night, 1.0) == pytest.approx(148)

    def test_a_file_names_itself_in_messages(self, tmp_path) -> None:
        path = tmp_path / "grip.yaml"
        path.write_text(GRIP.replace("low: [0.90, 0.65, 0.45]", "low: [0.90, 0.95, 0.45]"))
        with pytest.raises(RulesError, match=r"grip\.yaml: .*at water=50.0: low \(0.95\) must not be above high"):
            Rules.read(path)
        with pytest.raises(RulesError, match="cannot read rules"):
            Rules.read(tmp_path / "nowhere.yaml")
        with pytest.raises(RulesError, match=r"version 1 \(fuzzy rules\) is no longer read"):
            Rules.from_text(GRIP.replace("version: 2", "version: 1"))

    def test_unknown_quantities_and_missing_inputs_are_refused(self) -> None:
        rules = Rules.from_text(GRIP)
        with pytest.raises(RulesError, match="unknown quantity 'visibility'"):
            rules.evaluate("visibility", {})
        with pytest.raises(RulesError, match="reads water, which was not given"):
            rules.evaluate("grip", {})
        with_default = Rules.from_text(GRIP.replace("quantities:", "inputs:\n  water: {default: 100}\nquantities:"))
        assert with_default.input_defaults == {"water": 100.0}
        assert with_default.range("grip", {}) == pytest.approx((0.45, 0.65))

    def test_a_later_file_replaces_quantities_by_name(self, tmp_path) -> None:
        over = tmp_path / "over.yaml"
        over.write_text(GRIP.replace("0.45", "0.30"))
        merged = Rules.from_text(GRIP).merged(Rules.read(over))
        assert merged.range("grip", {"water": 100}) == pytest.approx((0.30, 0.65))
        assert merged.source("grip") == str(over)


@dataclass
class _WaterBackend(Backend):
    """Its rules read ``water``, the larger of ``wetness`` and ``deposits``."""

    def rule_values(self, values):  # type: ignore[no-untyped-def]
        out = super().rule_values(values)
        out.setdefault("water", max(out.get("wetness", 0.0), out.get("deposits", 0.0)))
        return out


class TestBackend:
    def test_a_backend_derives_with_its_rules(self) -> None:
        backend = _WaterBackend(rules=Rules.from_text(GRIP), rule_inputs=["water"])
        assert backend.derived_range("grip", {"wetness": 10, "deposits": 50}) == (
            pytest.approx((0.65, 0.85))
        )
        assert backend.derived("grip", {"water": 0}) == pytest.approx(
            {"low": 0.9, "high": 1.1, "median": 1.0, "mean": 1.0}
        )
        assert backend.derived_at("grip", {"wetness": 70}, 0.5) == pytest.approx(0.67)

    def test_a_run_reads_other_rules_over_the_backends_and_goes_back(self, tmp_path) -> None:
        backend = _WaterBackend(rules=Rules.from_text(GRIP), rule_inputs=["water"])
        over = tmp_path / "measured.yaml"
        over.write_text(GRIP.replace("0.45", "0.30"))

        assert backend.use_rules(over) is backend.active_rules
        assert backend.derived_range("grip", {"water": 100}) == pytest.approx((0.30, 0.65))
        assert backend.active_rules.source("grip") == str(over)

        assert backend.use_rules(None) is backend.rules
        assert backend.derived_range("grip", {"water": 100}) == pytest.approx((0.45, 0.65))

    def test_rules_may_read_only_the_backends_inputs(self, tmp_path) -> None:
        with pytest.raises(RulesError, match="unknown input 'water'"):
            Backend(rules=Rules.from_text(GRIP), rule_inputs=["rain"])
        backend = Backend(rule_inputs=["rain"])
        path = tmp_path / "grip.yaml"
        path.write_text(GRIP)
        with pytest.raises(RulesError, match="unknown input 'water'"):
            backend.use_rules(path)
        assert backend.active_rules is None

    def test_a_backend_without_rules_has_nothing_to_derive_until_a_run_gives_some(
        self, tmp_path
    ) -> None:
        backend = Backend(name="log")
        with pytest.raises(RulesError, match="the log backend has no rules"):
            backend.derived_range("grip", {"water": 0})
        path = tmp_path / "grip.yaml"
        path.write_text(GRIP)
        backend.use_rules(path)
        assert backend.derived_range("grip", {"water": 0}) == pytest.approx((0.9, 1.1))


OVER = """
version: 2
extends: {base}
quantities:
  grip:
    parts:
      water:
        uniform: {{low: [0.95, 0.60, 0.30], high: [1.05, 0.80, 0.50]}}
"""


class TestExtends:
    def test_a_file_extends_a_relative_path_writing_only_its_change(self, tmp_path) -> None:
        (tmp_path / "base.yaml").write_text(GRIP)
        (tmp_path / "vehicles").mkdir()
        mini = tmp_path / "vehicles" / "mini.yaml"
        mini.write_text(OVER.format(base="../base.yaml"))
        rules = Rules.read(mini)
        assert rules.range("grip", {"water": 100}) == pytest.approx((0.30, 0.50))
        # The points are base.yaml's.
        assert rules.range("grip", {"water": 25}) == pytest.approx((0.775, 0.925))
        assert [Path(p).name for p, _ in rules.files] == ["base.yaml", "mini.yaml"]

    def test_a_url_is_fetched_and_its_relative_paths_read_against_it(self) -> None:
        pages = {
            "https://example.com/rules/base.yaml": GRIP,
            "https://example.com/rules/vehicles/mini.yaml": OVER.format(base="../base.yaml"),
        }
        asked: list[str] = []

        def fetch(url: str) -> str:
            asked.append(url)
            return pages[url]

        rules = Rules.read("https://example.com/rules/vehicles/mini.yaml", fetch=fetch)
        assert rules.range("grip", {"water": 100}) == pytest.approx((0.30, 0.50))
        assert asked == [
            "https://example.com/rules/vehicles/mini.yaml",
            "https://example.com/rules/base.yaml",
        ]
        assert rules.files[0][0] == "https://example.com/rules/base.yaml"

    def test_a_failing_fetch_and_plain_http_are_refused(self, tmp_path) -> None:
        def fetch(url: str) -> str:
            raise OSError("404")

        with pytest.raises(RulesError, match="cannot read rules https://example.com/b.yaml"):
            Rules.from_text(OVER.format(base="https://example.com/b.yaml"), fetch=fetch)
        with pytest.raises(RulesError, match="only https:// URLs are read"):
            Rules.from_text(OVER.format(base="http://example.com/b.yaml"))

    def test_text_reads_what_it_extends_against_its_base(self, tmp_path) -> None:
        (tmp_path / "base.yaml").write_text(GRIP)
        rules = Rules.from_text(OVER.format(base="base.yaml"), "edited", base=tmp_path / "x.yaml")
        assert rules.range("grip", {"water": 100}) == pytest.approx((0.30, 0.50))

    def test_a_run_reads_a_url_over_the_backends_rules(self, monkeypatch) -> None:
        import openodd.rules

        monkeypatch.setattr(
            openodd.rules, "fetch_url", lambda url: OVER.format(base="base.yaml").replace(
                "extends: base.yaml\n", ""
            )
        )
        backend = _WaterBackend(rules=Rules.from_text(GRIP), rule_inputs=["water"])
        backend.use_rules("https://example.com/measured.yaml")
        assert backend.derived_range("grip", {"water": 100}) == pytest.approx((0.30, 0.50))


class TestManifest:
    def test_a_backend_describes_its_probes_knobs_and_rule_inputs(self) -> None:
        from openodd.sampler import OddKnob

        def rain(world):  # type: ignore[no-untyped-def]
            """Rain, from a 0-100 precipitation."""
            return None

        backend = Backend(
            name="sim",
            probes={"rain": (rain, ""), "speed_kph": (lambda w: None, "km/h")},
            readers={"measure": lambda v: (lambda w: None, "m")},
            knobs={
                rain: OddKnob("environment.rain", values={"none": 0, "heavy": [70, 100]}),
                "speed_kph": OddKnob("ego.speed", range=(0, 130), limits=(0, None)),
            },
            rules=Rules.from_text(GRIP, "grip.yaml"),
            rule_inputs=["water", "precipitation"],
            rule_input_knobs={"precipitation": "environment.rain"},
            rule_input_text={"water": "the larger of wetness and deposits"},
        )
        m = backend.manifest()
        assert m["openodd_backend"] == 1 and m["name"] == "sim"
        assert m["probes"]["rain"] == {"unit": "", "text": "Rain, from a 0-100 precipitation."}
        assert m["probes"]["speed_kph"] == {"unit": "km/h"}
        assert m["readers"] == ["measure"]
        assert m["knobs"]["rain"] == {
            "key": "environment.rain",
            "values": {"none": 0, "heavy": [70.0, 100.0]},
        }
        assert m["knobs"]["speed_kph"] == {
            "key": "ego.speed",
            "range": [0.0, 130.0],
            "limits": [0.0, None],
        }
        assert m["rule_inputs"] == {
            "water": {"text": "the larger of wetness and deposits"},
            "precipitation": {"knob": "environment.rain"},
        }
        assert m["rules"][0]["file"] == "grip.yaml"
        assert len(m["rules"][0]["sha256"]) == 64
        # It is plain data: JSON writes it as it is.
        import json

        assert json.loads(json.dumps(m)) == m


CHAIN = """version: 2
quantities:
  sight_speed:
    parts:
      sight:
        at: {visibility: [0, 400]}
        uniform: {low: [0, 20], high: [0, 40]}
  visibility:
    parts:
      fog:
        at: {fog: [0, 100]}
        uniform: {low: [200, 0], high: [400, 200]}
"""


class TestChains:
    def test_a_part_reads_another_quantity(self) -> None:
        rules = Rules.from_text(CHAIN)
        assert rules.order == ["visibility", "sight_speed"]
        assert rules.upstream("sight_speed") == ["visibility"]
        assert rules.free_inputs == ["fog"]
        # fog 50: the visibility's median, 200 m, gives 10-20.
        assert rules.range("sight_speed", {"fog": 50}) == pytest.approx((10, 20))
        # The visibility at its own quantile: 100 m.
        assert rules.range(
            "sight_speed", {"fog": 50}, quantiles={"visibility": 0.0}
        ) == pytest.approx((5, 10))
        # A visibility given is observed.
        assert rules.quantile(
            "sight_speed", {"fog": 50, "visibility": 400}, 1.0
        ) == pytest.approx(40)

    def test_sample_gives_every_quantity(self) -> None:
        rules = Rules.from_text(CHAIN)
        assert rules.sample({"fog": 50}, {"sight_speed": 1.0}) == pytest.approx(
            {"visibility": 200.0, "sight_speed": 20.0}
        )
        backend = Backend(rules=rules, rule_inputs=["fog"])
        assert backend.derived_sample({"fog": 50})["visibility"] == pytest.approx(200)
        assert backend.derived_at(
            "sight_speed", {"fog": 50}, 0.0, {"visibility": 1.0}
        ) == pytest.approx(15)

    def test_a_cycle_is_refused(self) -> None:
        text = (
            "version: 2\nquantities:\n"
            "  a: {parts: {p: {at: {b: [0, 1]}, uniform: {low: 0, high: 1}}}}\n"
            "  b: {parts: {p: {at: {a: [0, 1]}, uniform: {low: 0, high: 1}}}}\n"
        )
        with pytest.raises(RulesError, match="cycle: a -> b -> a"):
            Rules.from_text(text)


TAXONOMY = """
TAXONOMY:
  weather:
    rainfall_rate: float precipitation_rate
    rain: [none, some]
"""

GRIP_V3 = """
version: 3
taxonomy: taxonomy.yaml
basis: {kind: expert}
inputs:
  rainfall_rate: {concept: weather.rainfall_rate, unit: mm/h}
quantities:
  grip:
    parts:
      rain:
        at: {rainfall_rate: [0, 10]}
        uniform: {low: [0.9, 0.5], high: [1.1, 0.7]}
"""


class TestInputsAndTheTaxonomy:
    def test_version_3_inputs_stand_for_taxonomy_concepts(self, tmp_path: Path) -> None:
        (tmp_path / "taxonomy.yaml").write_text(TAXONOMY)
        (tmp_path / "grip.yaml").write_text(GRIP_V3)
        rules = Rules.read(tmp_path / "grip.yaml")
        assert rules.version == 3
        assert rules.taxonomy is not None and rules.taxonomy[0].endswith("taxonomy.yaml")
        assert rules.input_concepts == {"rainfall_rate": ("weather.rainfall_rate", "mm/h")}
        assert rules.unclassified_inputs == []

    def test_version_3_refuses_an_input_that_is_no_concept(self, tmp_path: Path) -> None:
        (tmp_path / "taxonomy.yaml").write_text(TAXONOMY)
        # A categorical concept is read at its labels, not at numbers.
        (tmp_path / "grip.yaml").write_text(
            GRIP_V3.replace("weather.rainfall_rate, unit: mm/h", "weather.rain")
        )
        with pytest.raises(RulesError, match="rainfall_rate is categorical: write its points as labels"):
            Rules.read(tmp_path / "grip.yaml")

    def test_version_2_inputs_are_unclassified(self) -> None:
        with pytest.warns(UnclassifiedInputsWarning, match=r"version 2 rules read inputs .*\(water\)"):
            rules = Rules.from_text(GRIP)
        assert rules.version == 2
        assert rules.unclassified_inputs == ["water"]


class TestBasis:
    def test_each_part_says_what_it_rests_on(self, tmp_path: Path) -> None:
        (tmp_path / "taxonomy.yaml").write_text(TAXONOMY)
        text = GRIP_V3.replace(
            "        uniform: {low: [0.9, 0.5], high: [1.1, 0.7]}\n",
            "        uniform: {low: [0.9, 0.5], high: [1.1, 0.7]}\n"
            "        basis: {kind: fitted, data: {file: drives.csv, rows: 120}}\n",
        )
        (tmp_path / "grip.yaml").write_text(text)
        rules = Rules.read(tmp_path / "grip.yaml")
        assert rules.parts("grip") == ["rain"]
        assert rules.basis("grip", "rain") == {
            "kind": "fitted",
            "data": {"file": "drives.csv", "rows": 120},
        }
        (tmp_path / "grip.yaml").write_text(GRIP_V3)
        assert Rules.read(tmp_path / "grip.yaml").bases() == {("grip", "rain"): {"kind": "expert"}}

    def test_version_2_parts_are_unstated(self) -> None:
        rules = Rules.from_text(GRIP)
        assert rules.basis("grip", "water") == {"kind": "unstated"}
        with pytest.raises(RulesError, match="no part 'nope'"):
            rules.basis("grip", "nope")

    def test_version_3_refuses_a_part_resting_on_nothing(self, tmp_path: Path) -> None:
        (tmp_path / "taxonomy.yaml").write_text(TAXONOMY)
        (tmp_path / "grip.yaml").write_text(GRIP_V3.replace("basis: {kind: expert}\n", ""))
        with pytest.raises(RulesError, match="no basis"):
            Rules.read(tmp_path / "grip.yaml")


def test_the_manifest_says_which_inputs_are_causes(tmp_path: Path) -> None:
    (tmp_path / "taxonomy.yaml").write_text(TAXONOMY)
    (tmp_path / "grip.yaml").write_text(GRIP_V3)
    backend = Backend(rules=Rules.read(tmp_path / "grip.yaml"), rule_inputs=["rainfall_rate"])
    manifest = backend.manifest()
    assert manifest["rule_inputs"]["rainfall_rate"] == {"concept": "weather.rainfall_rate", "unit": "mm/h"}
    assert manifest["taxonomy"]["file"].endswith("taxonomy.yaml")
    plain = Backend(rules=Rules.from_text(GRIP), rule_inputs=["water"]).manifest()
    assert plain["rule_inputs"]["water"] == {} and "taxonomy" not in plain


ROOT = """version: 2
quantities:
  gap:
    parts:
      base:
        uniform: {low: 2, high: 30}
"""


def test_a_quantity_reading_no_input(tmp_path: Path) -> None:
    """A part with no `at` reads no input: a root of the network, such as
    what a scenario sets up (the gap to a vehicle ahead)."""
    rules = Rules.from_text(ROOT)
    assert rules.inputs("gap") == [] and rules.part_axes("gap", "base") == []
    assert rules.range("gap", {}) == (2.0, 30.0)
    assert rules.quantile("gap", {}, 0.25) == pytest.approx(9.0)
    assert rules.sample({}, {"gap": 0.5}) == {"gap": pytest.approx(16.0)}
    # A fit keeps it a root, and writes it so.
    fit = rules.fit([{"gap": 5.0 + 0.5 * i} for i in range(40)])
    fit.write(tmp_path / "fit.yaml")
    again = Rules.read(tmp_path / "fit.yaml")
    assert again.part_axes("gap", "base") == []
    assert again.range("gap", {})[0] == pytest.approx(5.0, abs=1e-6)


# A scenario's own nodes: the gap a cut-in is staged at, longer in the wet
# (it reads the shared network's grip), and a root.
SCENARIO = """version: 2
quantities:
  vehicle_ahead_gap:
    parts:
      grip:
        at: {grip: [0.45, 1.1]}
        uniform: {low: [5, 2], high: [40, 30]}
  npc_speed_offset:
    parts:
      base:
        uniform: {low: -15, high: 15}
"""


def test_a_scenario_adds_nodes_to_the_shared_network(tmp_path: Path) -> None:
    shared, added = Rules.from_text(GRIP), Rules.from_text(SCENARIO)
    joined = shared.extended(added)
    assert joined.quantities == ["grip", "vehicle_ahead_gap", "npc_speed_offset"]
    assert joined.upstream("vehicle_ahead_gap") == ["grip"]
    # The gap at the grip the water leaves: dry (1.1 at the top) or soaked.
    assert joined.sample({"water": 0}, {"grip": 1.0, "vehicle_ahead_gap": 0.0})["vehicle_ahead_gap"] == pytest.approx(2.0)
    assert joined.sample({"water": 100}, {"grip": 0.0, "vehicle_ahead_gap": 0.0})["vehicle_ahead_gap"] == pytest.approx(5.0)
    # A shared node is not replaced: redefining one is refused.
    with pytest.raises(RulesError, match="defines grip, which the rules it adds to define"):
        shared.extended(Rules.from_text(GRIP.replace("0.90", "0.95")))
    # Nor an input described otherwise.
    with_default = "version: 2\ninputs:\n  water: {default: 10}\n" + GRIP.split("version: 2\n", 1)[1]
    with pytest.raises(RulesError, match="inputs.water: describes the input otherwise"):
        Rules.from_text(with_default).extended(
            Rules.from_text("version: 2\ninputs:\n  water: {default: 20}\n" + SCENARIO.split("version: 2\n", 1)[1])
        )


def test_a_backend_holds_a_scenarios_nodes(tmp_path: Path) -> None:
    (tmp_path / "cut_in.yaml").write_text(SCENARIO)
    backend = Backend(rules=Rules.from_text(GRIP))
    active = backend.add_rules(tmp_path / "cut_in.yaml")
    assert active is backend.active_rules and "vehicle_ahead_gap" in active.quantities
    assert backend.active_rules is active  # joined once
    # A run's rules over the shared ones keep the scenario's nodes.
    backend.use_rules(Rules.from_text(GRIP.replace("0.90", "0.95")))
    assert "vehicle_ahead_gap" in backend.active_rules.quantities
    assert backend.active_rules.range("grip", {"water": 0})[0] == pytest.approx(0.95)
    # A scenario that redefines a shared node is refused when it is added.
    (tmp_path / "bad.yaml").write_text(GRIP)
    with pytest.raises(RulesError, match="defines grip"):
        backend.add_rules(tmp_path / "bad.yaml")
    assert backend.add_rules(None).quantities == ["grip"]


# A version 3 scenario: its gap reads the shared grip, which it names no
# concept for, and its own basis.
SCENARIO_V3 = """
version: 3
taxonomy: taxonomy.yaml
basis: {kind: expert, by: scenario team}
quantities:
  vehicle_ahead_gap:
    parts:
      grip:
        at: {grip: [0.5, 1.1]}
        uniform: {low: [5, 2], high: [40, 30]}
"""


def test_a_version_3_scenario_reads_the_shared_nodes_and_keeps_its_basis(tmp_path: Path) -> None:
    (tmp_path / "taxonomy.yaml").write_text(TAXONOMY)
    (tmp_path / "grip.yaml").write_text(GRIP_V3.replace("{kind: expert}", "{kind: expert, by: weather team}"))
    (tmp_path / "cut_in.yaml").write_text(SCENARIO_V3)
    shared = Rules.read(tmp_path / "grip.yaml")
    # On its own it is no network: grip stands for no concept.
    with pytest.raises(RulesError, match="'grip' is read by a part but stands for no taxonomy concept"):
        Rules.read(tmp_path / "cut_in.yaml")
    joined = shared.read_extending(tmp_path / "cut_in.yaml")
    assert joined.upstream("vehicle_ahead_gap") == ["grip"]
    # Each side keeps what its parts rest on.
    assert joined.bases() == {
        ("grip", "rain"): {"kind": "expert", "by": "weather team"},
        ("vehicle_ahead_gap", "grip"): {"kind": "expert", "by": "scenario team"},
    }
    backend = Backend(rules=shared)
    assert backend.add_rules(tmp_path / "cut_in.yaml").bases() == joined.bases()


def test_a_scenarios_nodes_read_only_what_the_backend_gives(tmp_path: Path) -> None:
    backend = Backend(rules=Rules.from_text(GRIP), rule_inputs=["water"])
    wind = SCENARIO.replace("at: {grip: [0.45, 1.1]}", "at: {wind: [0, 20]}")
    with pytest.raises(RulesError, match="wind"):
        backend.add_rules(Rules.from_text(wind))
    assert backend.add_rules(Rules.from_text(SCENARIO)).quantities[-1] == "npc_speed_offset"


def test_a_scenario_node_may_not_take_a_shared_input(tmp_path: Path) -> None:
    """An added quantity named like an input a shared node reads would change
    how the shared node is worked out."""
    water = "version: 2\nquantities:\n  water:\n    parts:\n      base:\n        uniform: {low: 0, high: 100}\n"
    with pytest.raises(RulesError, match="defines water, which the rules it adds to define or read"):
        Rules.from_text(GRIP).extended(Rules.from_text(water))


def test_dropping_a_runs_rules_keeps_the_scenarios_nodes_joined() -> None:
    """A scenario that reads a node only a run's rules give: dropping those
    rules is refused, and the backend stays as it was."""
    backend = Backend(rule_inputs=["water"])
    backend.use_rules(Rules.from_text(GRIP))
    backend.add_rules(Rules.from_text(SCENARIO))
    with pytest.raises(RulesError, match="grip"):
        backend.use_rules(None)
    assert "vehicle_ahead_gap" in backend.active_rules.quantities
    assert "grip" in backend.active_rules.quantities


SUN = """version: 2
inputs:
  time_of_day: {labels: [day, dusk, night, dawn], default: day}
quantities:
  sun_altitude:
    parts:
      time:
        at: {time_of_day: [day, dusk, night, dawn]}
        uniform: {low: [10, -6, -30, -6], high: [70, 0, -6, 0]}
  sight:
    parts:
      light:
        at: {time_of_day: [day, night], water: [0, 100]}
        uniform: {low: [[250, 150], [50, 40]], high: [[400, 250], [150, 100]]}
"""


class TestCategoricalInputs:
    def test_a_part_is_written_at_a_categorical_inputs_labels(self) -> None:
        rules = Rules.from_text(SUN)
        assert rules.input_labels == {"time_of_day": ["day", "dusk", "night", "dawn"]}
        assert rules.range("sun_altitude", {"time_of_day": "dusk"}) == (-6.0, 0.0)
        assert rules.range("sun_altitude", {}) == (10.0, 70.0)  # the default, day
        # Nothing between two labels; a number axis beside one still interpolates.
        assert rules.range("sight", {"time_of_day": "night", "water": 50}) == pytest.approx((45.0, 125.0))
        assert rules.part_axes("sight", "light")[0][0] == "time_of_day"

    def test_a_label_the_part_is_not_written_at_is_refused(self) -> None:
        rules = Rules.from_text(SUN)
        with pytest.raises(RulesError, match="writes no distribution at time_of_day=dusk: it is written at day, night"):
            rules.range("sight", {"time_of_day": "dusk", "water": 0})
        with pytest.raises(RulesError, match="time_of_day is 'noon', not one of its labels"):
            rules.range("sun_altitude", {"time_of_day": "noon"})

    @pytest.mark.parametrize(
        ("change", "message"),
        [
            (("[day, dusk, night, dawn]}\n        uniform", "[dusk, day, night, dawn]}\n        uniform"), "write the labels once each, in time_of_day's order"),
            (("[day, dusk, night, dawn]}\n        uniform", "[day, noon, night, dawn]}\n        uniform"), "'noon' is not one of time_of_day's labels"),
            (("inputs:\n  time_of_day: {labels: [day, dusk, night, dawn], default: day}\n", ""), "read at labels, and time_of_day has none"),
            (("default: day", "default: noon"), "'noon' is not one of its labels"),
            (("time_of_day: [day, night], water", "time_of_day: [0, 2], water"), "time_of_day is categorical: write its points as labels"),
        ],
    )
    def test_labels_are_checked_against_the_input(self, change: tuple[str, str], message: str) -> None:
        with pytest.raises(RulesError, match=message):
            Rules.from_text(SUN.replace(*change))

    def test_a_version_3_input_takes_a_categorical_concepts_literals(self, tmp_path: Path) -> None:
        (tmp_path / "taxonomy.yaml").write_text(TAXONOMY)
        (tmp_path / "r.yaml").write_text(
            "version: 3\ntaxonomy: taxonomy.yaml\nbasis: {kind: expert}\n"
            "inputs:\n  rain: {concept: weather.rain}\n"
            "quantities:\n  carla_precipitation:\n    parts:\n      p:\n"
            "        at: {rain: [none, some]}\n        uniform: {low: [0, 20], high: [0, 90]}\n"
        )
        rules = Rules.read(tmp_path / "r.yaml")
        assert rules.input_labels == {"rain": ["none", "some"]}
        assert rules.range("carla_precipitation", {"rain": "some"}) == (20.0, 90.0)
        (tmp_path / "r.yaml").write_text(
            (tmp_path / "r.yaml").read_text().replace("{concept: weather.rain}", "{concept: weather.rain, unit: mm/h}")
        )
        with pytest.raises(RulesError, match="'weather.rain' is categorical: it has no unit"):
            Rules.read(tmp_path / "r.yaml")

    def test_a_fit_reads_labels_and_writes_them(self, tmp_path: Path) -> None:
        rules = Rules.from_text(SUN)
        rows = [{"time_of_day": "day", "sun_altitude": 20.0 + i} for i in range(20)]
        rows += [{"time_of_day": "night", "sun_altitude": -20.0 + i * 0.5} for i in range(20)]
        fit = rules.fit(rows, fixed=["sight"])
        fit.write(tmp_path / "fit.yaml")
        text = (tmp_path / "fit.yaml").read_text()
        assert '"time_of_day": ["day", "dusk", "night", "dawn"]' in text
        again = rules.read_over(tmp_path / "fit.yaml")
        low, high = again.range("sun_altitude", {"time_of_day": "day"})
        assert 19.0 < low < 21.0 and 38.0 < high < 40.0
        with pytest.raises(RulesError, match="not one of its labels"):
            rules.fit([{"time_of_day": "noon", "sun_altitude": 1.0}])


def test_a_fit_skips_what_a_categorical_cause_does_not_say() -> None:
    """An empty or NaN label is not observed (the default is read); a label a
    part is not written at says nothing of it, and is not put in a
    neighbour's distribution."""
    rules = Rules.from_text(SUN)
    day = [{"time_of_day": "day", "sight": 300.0 + i, "water": 0.0} for i in range(20)]
    night = [{"time_of_day": "night", "sight": 80.0 + i, "water": 0.0} for i in range(20)]
    # Dusk: sight has no distribution there; far off, it would drag day's.
    dusk = [{"time_of_day": "dusk", "sight": 5.0, "water": 0.0} for _ in range(20)]
    blank = [{"time_of_day": "", "sight": 310.0, "water": 0.0}, {"time_of_day": float("nan"), "sight": 305.0, "water": 0.0}]
    fit = rules.fit(day + night + dusk + blank, fixed=["sun_altitude"])
    low, _ = fit.rules.range("sight", {"time_of_day": "day", "water": 0})
    assert low > 290.0, low
