"""The case plot: the drawn cases of real collector output, and their HTML."""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

import pytest

import openodd
from openodd import CasePlot
from openodd.report import main

from test_flaky import BOUNDARY, passes, search
from test_sampling import odd


def _document(case: Any, space: Any, passed: bool, *, ticks: bool = True) -> dict[str, Any]:
    collector = openodd.CoverageCollector([], odd=odd() if ticks else None)
    collector.record_case(case, space=space)
    collector.record_outcome(passed)
    return json.loads(json.dumps(collector.to_dict("cut_in")))


def _campaign() -> list[dict[str, Any]]:
    """A sweep: 20 cases, half of them chosen for instability, and the first
    three run again -- the second of them with the other outcome."""
    s = search(explore_fraction=0.5)
    docs, first = [], []
    for _ in range(2):
        for case in s.sample(10):
            docs.append(_document(case, s.space, passes(case), ticks=False))
            s.observe(case, passes(case))
            first.append(case)
    for i, case in enumerate(first[:3]):
        docs.append(_document(case, s.space, passes(case) != (i == 1), ticks=False))
    blank = openodd.CoverageCollector([]).to_dict("cut_in")  # no case: not placed
    docs.append(json.loads(json.dumps(blank)))
    return docs


def test_the_cases_of_a_sweep() -> None:
    plot = CasePlot(_campaign())
    assert plot.attributes == ["rain", "gap"]
    assert plot.cases == 20 and plot.runs == 23 and plot.skipped == (1, 0)
    data = plot.data()
    assert data["x"]["name"] == "gap" and data["x"]["kind"] == "numeric"
    assert data["x"]["domain"] == [2.0, 35.0]
    assert data["y"]["name"] == "rain" and data["y"]["labels"] == ["none", "light", "heavy"]
    repeated = [c for c in data["cases"] if len(c["runs"]) > 1]
    assert len(repeated) == 3 and data["repeated"] == 3
    assert sorted(c["verdict"] for c in repeated if c["passes"] not in (0, 2)) == ["flaky"]
    assert data["counts"]["flaky"] == 1
    for c in data["cases"]:
        assert c["verdict"] == ("pass" if c["passes"] == len(c["runs"]) else
                                "fail" if c["passes"] == 0 else "flaky")
        if c["verdict"] != "flaky":
            assert (c["values"]["gap"] > BOUNDARY) == (c["verdict"] == "pass")
    chosen = [c for c in data["cases"] if c["instability"]]
    assert chosen and all(c["flaky"]["flip_probability"] is not None for c in chosen)


def test_the_fragment() -> None:
    plot = CasePlot(_campaign())
    html = plot.html(x="gap", y="rain", id="cp", link="/cases?{axes}",
                     labels={"title": "抽選ケース", "state_flaky": "不安定"})
    assert html.startswith('<div class="oo-cov oo-cases" id="cp" data-oo-mode="host">')
    assert "抽選ケース" in html and "不安定" in html
    assert 'data-oo-view="cases" data-oo-link="/cases?{axes}"' in html
    assert html.count('role="img" aria-label=') == 40  # 20 cases, wide and compact
    assert "oo-case oo-flaky" in html and "oo-instab" in html
    assert "Repeated cases" in html and "oo-o oo-o-fail" in html
    assert "1 coverage files without a case or an outcome" in html
    assert "Repeated cases" not in plot.html(table=False)
    with pytest.raises(ValueError, match="no such label"):
        plot.html(labels={"nope": "x"})


def test_the_table_and_the_explorer() -> None:
    plot = CasePlot(_campaign())
    table = plot.table_html(x="gap", y="rain")
    assert table.count("<tr class=\"oo-row-") == 3
    assert table.index('<tr class="oo-row-') == table.index('<tr class="oo-row-flaky')
    explorer = plot.explorer_html(x="gap", y="rain")
    assert explorer.count('<section class="oo-panel"') == 2  # gap x rain, rain x gap
    assert "data-oo-map=" in explorer
    page = plot.page("Cases", x="gap")
    assert page.startswith("<!doctype html>") and "oo-tips" in page and "openodd:axes" in page


def test_selection() -> None:
    plot = CasePlot(_campaign())
    assert plot.data(scenario="other")["cases"] == []
    assert len(plot.data(environment=None)["cases"]) == 20
    assert plot.data(environment="e")["cases"] == []


def test_runs_with_cells_and_without_a_case() -> None:
    s = search()
    case = s.sample(1)[0]
    docs = [_document(case, s.space, True)]
    plot = CasePlot(docs)
    assert plot.cases == 1
    # A view that only has the coverage view's data says why it is empty.
    empty = CasePlot([{"schema": "openodd.coverage/1", "scenario": "s", "items": []}])
    assert empty.cases == 0 and empty.skipped == (1, 0)
    assert "No case with drawn values" in empty.html()


def test_the_cli_writes_the_page(tmp_path: Path) -> None:
    for i, doc in enumerate(_campaign()):
        (tmp_path / f"r{i}_coverage.json").write_text(json.dumps(doc), encoding="utf-8")
    out = tmp_path / "out" / "cases.html"
    assert main([str(tmp_path), "--cases-html", str(out), "--cases-x", "rain", "--cases-y", "gap",
                 "--markdown", str(tmp_path / "r.md")]) == 0
    text = out.read_text(encoding="utf-8")
    assert 'data-oo-axis="x" data-oo-current="rain"' in text
    assert "oo-case oo-flaky" in text


def test_it_is_exported() -> None:
    assert openodd.CasePlot is CasePlot and ".oo-plot" in openodd.view.styles()
    assert "data-oo-tip-i" in openodd.view.script()
