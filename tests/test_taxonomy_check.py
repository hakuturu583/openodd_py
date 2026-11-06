"""What ODDs, bindings and rules use that a reference taxonomy lacks (#42)."""

from __future__ import annotations

from pathlib import Path

import pytest

from openodd.taxonomy_check import check_taxonomy, main

REFERENCE = """
TAXONOMY:
  weather:
    rainfall_rate: float precipitation_rate
    rain_intensity:
      none: {rainfall_rate: "< 0.1 mm/h"}
      some: {rainfall_rate: ">= 0.1 mm/h"}
  traffic:
    flow_rate: float traffic_rate
  road:
    speed_limit: float speed_km_per_h
"""

OURS = """
TAXONOMY:
  weather:
    rainfall_rate: float precipitation_rate
    rain_intensity: [none, light, heavy]
  traffic:
    flow_rate: integer count
  road:
    speed_limit: float velocity
    location: [urban, nonurban]
"""


@pytest.fixture
def files(tmp_path: Path) -> Path:
    (tmp_path / "reference.yaml").write_text(REFERENCE)
    (tmp_path / "ours.yaml").write_text(OURS)
    (tmp_path / "ours.binding.yaml").write_text(
        "openodd: [ours.yaml]\nprobes:\n  road.speed_limit: {probe: speed_kph}\n"
    )
    (tmp_path / "v2.yaml").write_text(
        "version: 2\nquantities:\n  grip:\n    parts:\n      water:\n"
        "        at: {water: [0, 100]}\n        uniform: {low: [0.9, 0.5], high: [1.1, 0.7]}\n"
    )
    (tmp_path / "v3.yaml").write_text(
        "version: 3\ntaxonomy: ours.yaml\nbasis: {kind: expert}\n"
        "inputs:\n  grade: {concept: road.speed_limit}\n  rate: {concept: weather.rainfall_rate}\n"
        "quantities:\n  grip:\n    parts:\n      rain:\n        at: {rate: [0, 10], grade: [0, 50]}\n"
        "        uniform: {low: 0.5, high: 1.0}\n"
    )
    return tmp_path


def test_what_the_files_use_and_the_reference_lacks(files: Path) -> None:
    result = check_taxonomy(
        files / "reference.yaml",
        odds=[files / "ours.binding.yaml"],
        rules=[files / "v2.yaml", files / "v3.yaml"],
    )
    found = {(f.kind, f.concept) for f in result.findings}
    assert found == {
        ("missing", "road.location"),
        ("differs", "weather.rain_intensity"),  # other literals
        ("differs", "traffic.flow_rate"),  # a count, not a traffic rate
        ("unclassified", "water"),
    }
    # speed_km_per_h and velocity are one dimension: no finding.
    assert not result.ok
    draft = result.draft(result.missing()[0])
    assert draft.startswith("# Add `road.location` to the taxonomy")
    assert "`urban`, `nonurban`" in draft


def test_the_command(files: Path, tmp_path: Path, capsys: pytest.CaptureFixture[str]) -> None:
    drafts = tmp_path / "drafts"
    code = main([str(files / "reference.yaml"), "--odd", str(files / "ours.yaml"), "--drafts", str(drafts)])
    assert code == 1
    out = capsys.readouterr().out
    assert "missing: road.location" in out and "3 finding(s)" in out
    assert (drafts / "road.location.md").exists()
    assert main([str(files / "reference.yaml"), "--odd", str(files / "reference.yaml")]) == 0
    assert main([str(files / "nope.yaml")]) == 2


def test_a_rule_inputs_concept_declared_otherwise_differs(files: Path) -> None:
    """#80: a rules-only check compares the rules' declarations too."""
    result = check_taxonomy(files / "reference.yaml", rules=[files / "v3.yaml"])
    found = {(f.kind, f.concept) for f in result.findings}
    # v3.yaml's taxonomy (ours.yaml) declares the rainfall rate as the reference does,
    # and the speed limit as a velocity (one dimension with speed_km_per_h): no finding.
    assert found == set()
    (files / "v3_flow.yaml").write_text(
        (files / "v3.yaml").read_text().replace("weather.rainfall_rate", "traffic.flow_rate")
    )
    result = check_taxonomy(files / "reference.yaml", rules=[files / "v3_flow.yaml"])
    assert [(f.kind, f.concept) for f in result.findings] == [("differs", "traffic.flow_rate")]


def test_a_binding_that_leaves_its_units_to_the_backend(tmp_path: Path) -> None:
    """#81: no backend says the probes' units; the check needs none."""
    (tmp_path / "reference.yaml").write_text(REFERENCE)
    (tmp_path / "odd.yaml").write_text(
        "TAXONOMY:\n  road:\n    speed_limit: float speed_km_per_h\nMODULES:\n  slow:\n"
        "    INCLUDE_AND:\n      road.speed_limit: \"< 50 km/h\"\n"
    )
    (tmp_path / "b.yaml").write_text("openodd: [odd.yaml]\nprobes:\n  road.speed_limit: {probe: speed_kph}\n")
    result = check_taxonomy(tmp_path / "reference.yaml", odds=[tmp_path / "b.yaml"])
    assert result.ok, result.findings
    assert main([str(tmp_path / "reference.yaml"), "--odd", str(tmp_path / "b.yaml")]) == 0


def test_the_rules_taxonomy_is_not_read_again(files: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    """A rules taxonomy's label (a URL, or ``url@rev:path`` for a git source)
    need not be readable as a path: the declarations kept when the rules were
    read are compared, and the taxonomy is fetched once."""
    import openodd.rules

    fetched: list[str] = []

    def fetch(url: str) -> str:
        fetched.append(url)
        return OURS.replace("integer count", "float traffic_rate")

    monkeypatch.setattr(openodd.rules, "fetch_url", fetch)
    (files / "v3_url.yaml").write_text(
        (files / "v3.yaml")
        .read_text()
        .replace("taxonomy: ours.yaml", "taxonomy: https://example.com/taxonomy.yaml")
        .replace("weather.rainfall_rate", "traffic.flow_rate")
    )
    result = check_taxonomy(files / "reference.yaml", rules=[files / "v3_url.yaml"])
    # The fetched taxonomy declares the flow rate as the reference does.
    assert result.findings == []
    assert fetched == ["https://example.com/taxonomy.yaml"]


def test_a_scenario_parameter_is_not_looked_for(tmp_path: Path) -> None:
    """#92: a binding's parameters stand for no taxonomy concept."""
    (tmp_path / "reference.yaml").write_text(REFERENCE)
    (tmp_path / "odd.yaml").write_text("TAXONOMY:\n  road:\n    speed_limit: float speed_km_per_h\n")
    (tmp_path / "b.yaml").write_text(
        "openodd: [odd.yaml]\nparameters:\n"
        "  scenario.lead_gap: {type: float length, measure: gap_m, buckets: [0, 10]}\n"
    )
    result = check_taxonomy(tmp_path / "reference.yaml", odds=[tmp_path / "b.yaml"])
    assert result.ok, result.findings
