"""Fuzzy rules (src/rules.rs, openodd.rules) and the backend that holds them."""

from __future__ import annotations

import json
from dataclasses import dataclass
from pathlib import Path

import pytest

from openodd import Backend, Rules, RulesError

GOLDEN = json.loads((Path(__file__).parent / "rules_golden.json").read_text())

GRIP = GOLDEN["files"]["grip"]


@pytest.mark.parametrize("case", GOLDEN["cases"], ids=lambda c: c["file"])
def test_the_rules_infer_as_simpful_does(case) -> None:
    rules = Rules.from_text(GOLDEN["files"][case["file"]], case["file"])
    for point in case["points"]:
        out = rules.evaluate(case["quantity"], point["inputs"])
        assert out == pytest.approx(point["outputs"], abs=1e-12), point


class TestRules:
    def test_a_range_reads_low_and_high(self) -> None:
        rules = Rules.from_text(GRIP)
        assert rules.quantities == ["grip"]
        assert rules.inputs("grip") == ["water"]
        assert rules.outputs("grip") == ["low", "high"]
        assert rules.range("grip", {"water": 25}) == pytest.approx((0.775, 0.975))

    def test_a_file_names_itself_in_messages(self, tmp_path) -> None:
        path = tmp_path / "grip.yaml"
        path.write_text(GRIP.replace("(water IS wet)", "(water IS soaked)"))
        with pytest.raises(RulesError, match=r"grip\.yaml: .*water has no term 'soaked'"):
            Rules.read(path)
        with pytest.raises(RulesError, match="cannot read rules"):
            Rules.read(tmp_path / "nowhere.yaml")

    def test_unknown_quantities_and_missing_inputs_are_refused(self) -> None:
        rules = Rules.from_text(GRIP)
        with pytest.raises(RulesError, match="unknown quantity 'visibility'"):
            rules.evaluate("visibility", {})
        with pytest.raises(RulesError, match="reads water, which was not given"):
            rules.evaluate("grip", {})

    def test_a_quantity_without_low_and_high_has_no_range(self) -> None:
        rules = Rules.from_text(GOLDEN["files"]["operators"])
        with pytest.raises(RulesError, match="not low and high"):
            rules.range("q", {"x": 0, "y": 0})

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
        assert backend.derived("grip", {"water": 0}) == pytest.approx({"low": 0.9, "high": 1.1})

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
version: 1
extends: {base}
quantities:
  grip:
    rule_bases:
      water:
        rules:
          - {{if: (water IS dry), then: {{low: 0.95, high: 1.05}}}}
          - {{if: (water IS wet), then: {{low: 0.60, high: 0.80}}}}
          - {{if: (water IS flooded), then: {{low: 0.30, high: 0.50}}}}
"""


class TestExtends:
    def test_a_file_extends_a_relative_path_writing_only_its_change(self, tmp_path) -> None:
        (tmp_path / "base.yaml").write_text(GRIP)
        (tmp_path / "vehicles").mkdir()
        mini = tmp_path / "vehicles" / "mini.yaml"
        mini.write_text(OVER.format(base="../base.yaml"))
        rules = Rules.read(mini)
        assert rules.range("grip", {"water": 100}) == pytest.approx((0.30, 0.50))
        # The inputs are base.yaml's.
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
