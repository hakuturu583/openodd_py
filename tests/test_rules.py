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
        (tmp_path / "grip.yaml").write_text(GRIP_V3.replace("weather.rainfall_rate", "weather.rain"))
        with pytest.raises(RulesError, match="is not a number"):
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
