"""The case plot: the drawn cases of real collector output, and their HTML."""

from __future__ import annotations

import json
import re
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
    assert "oo-case oo-flaky" in html and "oo-instab" not in html
    assert "oo-instab" in plot.html(instability_rings=True)
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
    explorer = plot.explorer_html(x="gap", y="rain", prediction=False)
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


def test_the_predicted_background() -> None:
    plot = CasePlot(_campaign())
    grid = plot.prediction(x="gap", y="rain")
    assert grid["runs"] == 23
    assert grid["x"]["name"] == "gap" and len(grid["x"]["edges"]) == 49
    assert grid["y"]["labels"] == ["none", "light", "heavy"]
    assert grid["held"] == []  # the space is gap and rain: nothing held
    assert grid["flip_threshold"] == 0.1 and grid["flaky_confidence"] == 0.5
    states = [c["state"] for row in grid["cells"] for c in row if c]
    # Fails where the gap is short, passes where it is long.
    first, last = grid["cells"][0][0], grid["cells"][0][-1]
    assert first["state"] == "fail" and first["pass_probability"] < 0.5
    assert last["pass_probability"] > 0.5
    right = [c for row in grid["cells"] for c in row[len(row) // 2:] if c]
    assert any(c["state"] == "pass" for c in right)
    assert set(states) <= {"pass", "fail", "flaky"}
    for c in (c for row in grid["cells"] for c in row if c):
        if c["instability_confidence"] > 0.5:
            assert c["state"] == "flaky"
        else:
            assert c["state"] == ("pass" if c["pass_probability"] >= 0.5 else "fail")
    data = plot.data(x="gap", y="rain")
    assert all(0 <= c["predicted_pass_probability"] <= 1 for c in data["cases"])
    assert "prediction" not in plot.data(prediction=False)
    html = plot.html(x="gap", y="rain")
    assert '<g class="oo-pred"' in html and "Predicted pass rate" in html
    # The colour is the pass probability, to the nearest 5%: the short gaps
    # (low p) and the long ones (high p) get different steps.
    steps = {int(k) for k in re.findall(r'<rect class="oo-pp-(\d+)"', html)}
    assert min(steps) < 10 < max(steps)
    ja = plot.html(x="gap", y="rain", labels={
        "predicted_scale": "予測成功率", "predicted_low": "低い", "predicted_mid": "不安定",
        "predicted_high": "高い"})
    assert "予測成功率" in ja and "50% 不安定" in ja and "100% 高い" in ja
    assert "fitted to these 23 runs" in html and "Pass probability (model, now)" in html
    assert "oo-pred" not in plot.html(x="gap", y="rain", prediction=False)


def test_one_outcome_has_no_background() -> None:
    s = search()
    docs = [_document(c, s.space, True, ticks=False) for c in s.sample(6)]
    plot = CasePlot(docs)
    assert "every run passed" in plot.prediction()["unavailable"]
    html = plot.html()
    assert '<g class="oo-pred"' not in html and "No predicted background: every run passed" in html
    few = CasePlot(docs[:2])
    assert "too few runs" in few.prediction()["unavailable"]


def _synthetic(n: int, *, name: str = "rain", value: str = "heavy") -> list[dict[str, Any]]:
    """*n* runs over four attributes, passing where the gap beats the speed."""
    import random

    rng = random.Random(7)
    labels = ["none", "light", value]
    space = [{"name": "gap", "range": [0.0, 40.0]}, {"name": "speed", "range": [0.0, 20.0]},
             {"name": "decel", "range": [1.0, 8.0]}, {"name": name, "labels": labels}]
    docs = []
    for _ in range(n):
        v = {"gap": rng.uniform(0, 40), "speed": rng.uniform(0, 20), "decel": rng.uniform(1, 8),
             name: rng.choice(labels)}
        docs.append({"scenario": "s", "outcome": {"passed": v["gap"] > v["speed"] + rng.gauss(0, 3)},
                     "case": {"values": v, "overrides": [f"{k}={x}" for k, x in v.items()],
                              "chosen_for": "coverage", "space": space}})
    return docs


def _scripts(page: str) -> int:
    """The <script> elements an HTML5 parser finds in *page*."""
    try:
        import html5lib
    except ImportError:  # the tokenizer's count, without the script states
        from html.parser import HTMLParser

        class Count(HTMLParser):
            n = 0

            def handle_starttag(self, tag: str, attrs: Any) -> None:
                self.n += tag == "script"

        c = Count()
        c.feed(page)
        return c.n
    doc = html5lib.parse(page, namespaceHTMLElements=False)
    return len(doc.findall(".//script"))


def test_script_markup_in_the_data_cannot_end_the_tips_block() -> None:
    # `<!--<script>` would put the parser in the double-escaped state, where
    # the block's own `</script>` does not close it.
    plot = CasePlot(_synthetic(12, name="<!--<script>", value="</script><!--<script>"))
    for body in (plot.html(x="<!--<script>", y="gap"), plot.explorer_html(x="gap", y="speed")):
        blocks = re.findall(r'<script type="application/json" class="oo-tips">(.*?)</script>',
                            body, re.S)
        assert len(blocks) == 1 and not re.search("[<>&]", blocks[0])
        assert any("</script><!--<script>" in json.dumps(c) for c in json.loads(blocks[0]))
        page = openodd.view.page("Cases", body)
        # The tips' JSON and the components' script, both closed.
        assert _scripts(page) == 2
        assert page.rstrip().endswith("</script></body></html>")


def test_tips_and_pairs_cost_one_fit() -> None:
    # The tips take the posterior pass probability (no Monte Carlo per case),
    # and the model and its grids are kept across calls and transposed
    # pairs: 120 runs over four attributes render well within the bound,
    # even unoptimized.
    import time

    plot = CasePlot(_synthetic(120))
    t = time.perf_counter()
    first = plot.html(x="gap", y="speed")
    plot.html(x="speed", y="gap")
    plot.explorer_html(x="gap", y="speed")
    assert time.perf_counter() - t < 60.0
    assert "Pass probability (model, now)" in first


#: The scenario stages the gap 2 m longer than it was drawn: a knob's offset
#: is off by 2 m. The rain is staged as drawn.
GAP_OFFSET = 2.0


def _staged(case: Any, space: Any, passed: bool) -> dict[str, Any]:
    """A run that ticks: what its probes measure is the drawn case, with the
    gap shifted by GAP_OFFSET and closing at 1 m/s."""
    from test_sampling import backend

    collector = openodd.CoverageCollector([], odd=odd(), backend=backend())
    gap, rain = case.values["gap"] + GAP_OFFSET, case.values["rain"]
    collector.start({"gap_m": gap, "rain": rain}, 0.0)
    t = 0.0
    for _ in range(30):
        t += 0.1
        collector.tick({"gap_m": gap - t, "rain": rain}, t)
    collector.end({}, t)
    collector.record_case(case, space=space)
    collector.record_outcome(passed)
    return json.loads(json.dumps(collector.to_dict("cut_in")))


def _staged_campaign() -> list[dict[str, Any]]:
    s = search(explore_fraction=0.5)
    docs = []
    for _ in range(2):
        for case in s.sample(10):
            docs.append(_staged(case, s.space, passes(case)))
            s.observe(case, passes(case))
    return docs


def test_the_drift_finds_the_knobs_offset() -> None:
    plot = CasePlot(_staged_campaign())
    drift = plot.drift()
    by = {a["attribute"]: a for a in drift["attributes"]}
    gap, rain = by["gap"], by["rain"]
    # The measured gap is the first sample, at t = 0.1 s: drawn + 2 - 0.1.
    assert gap["summary"]["runs"] == 20
    assert gap["summary"]["bias"] == pytest.approx(GAP_OFFSET - 0.1, abs=1e-6)
    assert gap["fit"]["a"] == pytest.approx(1.0, abs=1e-6)
    assert gap["fit"]["b"] == pytest.approx(GAP_OFFSET - 0.1, abs=1e-6)
    assert gap["calibration"]["status"] == "suggest"
    assert gap["calibration"]["write_as"]["offset"] == pytest.approx(-(GAP_OFFSET - 0.1), abs=1e-6)
    same = [r["drawn_bucket"] == r["measured_bucket"] for r in gap["runs"]]
    assert gap["summary"]["bucket_hit_rate"] == pytest.approx(sum(same) / len(same))
    assert all(r["measured_bucket"] == next(b["label"] for b in gap["buckets"]
                                            if b["low"] <= r["measured"] < b["high"]
                                            or (b is gap["buckets"][-1] and r["measured"] <= b["high"]))
               for r in gap["runs"])
    assert rain["summary"]["bucket_hit_rate"] == 1.0
    assert drift["attributes"][0]["attribute"] == "gap"  # the worst first
    # The window: nothing measured before the first sample.
    assert plot.drift(measured_at=0.05)["attributes"] == []
    knob = plot.drift(knobs={"gap": (1.0, 11.0)})["attributes"][0]["calibration"]["knob"]
    assert knob["corrected"]["scale"] == pytest.approx(1.0)
    assert knob["corrected"]["offset"] == pytest.approx(11.0 - (GAP_OFFSET - 0.1))
    html = plot.drift_html(knobs={"gap": (1.0, 11.0)})
    assert "Scenario realisation" in html and 'class="oo-fit"' in html
    assert "Write the drawn value d as (d − 1.9 m) / 1.000" in html
    assert "scale 1.000, offset 9.100" in html
    with pytest.raises(ValueError, match="knob"):
        plot._core.drift_html(plot._query(None, None, None, openodd.view.ANY), "", None,
                              json.dumps({"knobs": {"gap": [1]}}))


def test_the_measured_basis() -> None:
    plot = CasePlot(_staged_campaign())
    drawn = plot.data(x="gap", y="rain", prediction=False)
    measured = plot.data(x="gap", y="rain", basis="measured", prediction=False)
    assert measured["basis"] == "measured"
    for d, m in zip(drawn["cases"], measured["cases"]):
        assert m["values"]["gap"] == pytest.approx(d["values"]["gap"] + GAP_OFFSET - 0.1)
        assert m["other_values"]["gap"] == d["values"]["gap"]
    # The model is fitted on the measured coordinates.
    assert plot.prediction(x="gap", y="rain", basis="measured")["runs"] == 20
    html = plot.html(x="gap", y="rain", drift_leaders=True)
    assert 'class="oo-basis"' in html and 'data-oo-basis-current="drawn"' in html
    assert 'class="oo-drift-lead"' in html and "Line to the measured value" in html
    assert "Placed by measured values" in plot.html(x="gap", y="rain", basis="measured")
    explorer = plot.explorer_html(x="gap", y="rain", prediction=False)
    assert "\\u001fmeasured" in explorer or "\u001fmeasured" in explorer
    with pytest.raises(ValueError, match="basis"):
        plot.html(basis="aimed")


def test_the_cli_writes_the_drift_page(tmp_path: Path) -> None:
    for i, doc in enumerate(_staged_campaign()):
        (tmp_path / f"r{i}_coverage.json").write_text(json.dumps(doc), encoding="utf-8")
    out = tmp_path / "drift.html"
    cases = tmp_path / "cases.html"
    assert main([str(tmp_path), "--drift-html", str(out), "--knob", "gap=1,11",
                 "--cases-html", str(cases), "--cases-basis", "measured", "--measured-at", "0.5",
                 "--markdown", str(tmp_path / "r.md")]) == 0
    assert "offset 9.100" in out.read_text(encoding="utf-8")
    assert "Placed by measured values" in cases.read_text(encoding="utf-8")
