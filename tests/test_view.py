"""The coverage view: matrices of real collector output, and their HTML."""

from __future__ import annotations

import json
import random
from pathlib import Path
from typing import Any, Optional

import pytest

import openodd
from openodd import CoverageCollector, CoverageView, OddAttribute, OddDefinition, OddModule
from openodd.report import main
from openodd.view import parse_conditions


def _odd() -> OddDefinition:
    speed = OddAttribute("speed", lambda c: c["speed"], unit="km/h", buckets=[0, 30, 60, 90])
    rain = OddAttribute("rain", lambda c: c["rain"], values=["none", "light", "heavy"])
    light = OddAttribute("light", lambda c: c["light"], values=["day", "night"])
    root = OddModule("root", include_and=[speed.less_than(60)])  # 60+ is outside
    return OddDefinition("urban", [speed, rain, light], [root])


def _run(odd: OddDefinition, steps: list[tuple[float, str, str]], passed: Optional[bool],
         failed_at: Optional[float] = None, environment: Optional[str] = None) -> dict[str, Any]:
    c = CoverageCollector([], odd=odd)
    c.start({"speed": None, "rain": None, "light": None}, 0.0)
    t = 0.0
    for speed, rain, light in steps:
        for _ in range(10):
            t += 0.1
            c.tick({"speed": speed, "rain": rain, "light": light}, t)
    c.end({}, t)
    if passed is not None:
        c.record_outcome(passed, failed_at=failed_at, environment=environment)
    return json.loads(json.dumps(c.to_dict("cut_in")))


def _campaign() -> list[dict[str, Any]]:
    odd, rng = _odd(), random.Random(3)
    docs = []
    for i in range(12):
        rain = ["none", "light", "heavy"][i % 3]
        # Slow, then fast. Heavy rain always fails once fast; light rain one in two.
        fails = rain == "heavy" or (rain == "light" and i % 2 == 1)
        docs.append(_run(odd, [(20.0, rain, "day"), (45.0, rain, "day")], not fails,
                         failed_at=1.5 + rng.random() * 0.4 if fails else None))
    docs.append(_run(odd, [(20.0, "none", "night")], None))  # no outcome
    return docs


def test_the_matrix_of_a_campaign() -> None:
    view = CoverageView(_campaign())
    assert view.attributes == ["speed", "rain", "light"]
    assert view.runs == 13 and view.scenarios == ["cut_in"] and view.environments == [None]
    m = view.matrix()
    assert (m["x"]["name"], m["y"]["name"]) == ("speed", "rain")
    assert m["x"]["outside_odd"] == ["[60, 90]"]
    xs, ys = m["x"]["labels"], m["y"]["labels"]
    cell = lambda y, x: m["cells"][ys.index(y)][xs.index(x)]  # noqa: E731
    assert cell("heavy", "[30, 60)")["state"] == "fail"
    assert cell("heavy", "[30, 60)")["fail_at_cell"] == 4
    assert cell("heavy", "[0, 30)")["state"] == "fail" and cell("heavy", "[0, 30)")["fail_at_cell"] == 0
    assert cell("light", "[30, 60)")["state"] == "flaky"
    assert cell("none", "[30, 60)")["state"] == "pass"
    assert cell("none", "[60, 90]")["state"] == "outside"
    assert cell("light", "[60, 90]")["state"] == "outside"
    assert m["total"]["runs"] == 12 and m["total"]["runs_unknown"] == 1
    # Inside a cell: the attribute left.
    inner = view.matrix(conditions=[("speed", "[30, 60)"), ("rain", "light")])
    assert inner["x"]["name"] == "light" and inner["y"] is None
    assert inner["cells"][0][inner["x"]["labels"].index("day")]["state"] == "flaky"


def test_thresholds_and_selection_reach_the_core() -> None:
    view = CoverageView(_campaign())
    m = view.matrix(min_runs=50)
    assert all(c["tentative"] for row in m["cells"] for c in row if c["runs"])
    assert view.matrix(environment="nope")["total"]["runs"] == 0
    assert view.matrix(environment=None)["total"]["runs"] == 12
    with pytest.raises(ValueError, match="band"):
        view.matrix(band=(0.9, 0.1))
    with pytest.raises(ValueError, match="min_runs"):
        view.matrix(min_runs=0)
    assert view.matrix(conditions={"speed": "[0, 30)"})["conditions"] == [
        {"attribute": "speed", "bucket": "[0, 30)"}]


def test_the_fragment_links_back_with_its_conditions() -> None:
    view = CoverageView(_campaign())
    html = view.html(link="/coverage?{conditions}", id="cv", labels={"title": "ODD カバレッジ"})
    assert html.startswith('<div class="oo-cov" id="cv" data-oo-mode="host">')
    assert "ODD カバレッジ" in html
    assert 'href="/coverage?at=rain%3Dheavy&amp;at=speed%3D%5B30%2C%2060%29"' in html
    href = "at=rain%3Dheavy&at=speed%3D%5B30%2C%2060%29"
    from urllib.parse import parse_qs

    assert parse_conditions(parse_qs(href)["at"]) == [("rain", "heavy"), ("speed", "[30, 60)")]
    with pytest.raises(ValueError, match="no such label"):
        view.html(labels={"nope": "x"})


def test_the_explorer_and_the_page() -> None:
    view = CoverageView(_campaign())
    html = view.explorer_html(depth=1)
    assert html.count('<section class="oo-panel"') > 1
    assert 'data-oo-open="oo-cov-p1"' in html
    page = view.page("Coverage", depth=1)
    assert page.startswith("<!doctype html>") and "openodd:drill" in page and ".oo-cov" in page
    with pytest.raises(ValueError):
        view.explorer_html(depth=-1)


def test_the_cli_writes_the_page(tmp_path: Path) -> None:
    for i, doc in enumerate(_campaign()):
        (tmp_path / f"r{i}_coverage.json").write_text(json.dumps(doc), encoding="utf-8")
    out = tmp_path / "out" / "coverage.html"
    assert main([str(tmp_path), "--html", str(out), "--html-depth", "1",
                 "--markdown", str(tmp_path / "r.md")]) == 0
    text = out.read_text(encoding="utf-8")
    assert "oo-panel" in text and "oo-fail" in text


def test_files_without_cells_or_outcomes_still_render() -> None:
    view = CoverageView([{"schema": "openodd.coverage/1", "scenario": "s", "items": []}])
    assert view.attributes == [] and view.matrix()["x"] is None
    assert "No ODD attribute" in view.html()


def test_it_is_exported() -> None:
    assert openodd.CoverageView is CoverageView and openodd.view.styles()
