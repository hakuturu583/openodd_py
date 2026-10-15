"""The test report: coverage files as a page, and Claude's analysis of them."""

from __future__ import annotations

import asyncio
import json
from pathlib import Path
from typing import Any

import pytest

import openodd
from openodd import ReportAnalysis, TestReport
from openodd.testreport import main

from test_case_plot import _campaign as _sweep
from test_view import _campaign


def _report() -> TestReport:
    return TestReport(_campaign() + _sweep(), title="Nightly")


def test_the_figures() -> None:
    s = _report().summary()
    assert s["title"] == "Nightly"
    assert s["runs"] == 13 + 24
    assert s["passed"] + s["failed"] + s["no_outcome"] == s["runs"]
    view = [r for r in s["scenarios"] if r["scenario"] == "cut_in"][0]
    assert view["runs"] == s["runs"]
    assert s["pass_rate"] == pytest.approx(s["passed"] / (s["passed"] + s["failed"]))
    # The view's campaign: heavy rain always fails, light one in two.
    assert sum(1 for f in s["failures"] if f["failed_at"] is not None) == 6
    assert all(f["name"].startswith("#") for f in s["failures"])
    items = {i["name"]: i for i in s["coverage"]["items"]}
    assert set(items) == {"odd.speed", "odd.rain", "odd.light"} and items["odd.rain"]["grade"] == 1.0
    assert s["odds"][0]["name"] == "urban"
    assert any(c["labels"] for c in s["failing_cells"])
    assert s["cases_recorded"] == 23
    assert all(c["instability_confidence"] > 0.5 for c in s["flaky_cases"])
    json.dumps(s)  # plain data


def test_the_page() -> None:
    report = _report()
    page = report.page()
    assert page.startswith("<!doctype html>") and "<title>Nightly</title>" in page
    for heading in ("Analysis", "Results", "Failed runs (", "Flakiness", "ODD matrix",
                    "Drawn cases", "Scenario realisation"):
        assert f"<h2>{heading}" in page, heading
    assert "No analysis was made" in page
    assert "ODD matrix" not in report.html(views=False)
    import html5lib

    html5lib.HTMLParser(strict=False).parse(page)


def test_an_analysis_on_the_page() -> None:
    analysis = ReportAnalysis.from_dict({
        "verdict": "FAIL",
        "headline": "Heavy rain <fails>",
        "summary": "Six runs failed.",
        "findings": [
            {"severity": "minor", "title": "light rain flaky", "detail": "1 in 2", "evidence": ["4/8"]},
            {"severity": "critical", "title": "heavy rain", "detail": "every run", "evidence": []},
            {"severity": "bogus", "title": "x", "detail": "y"},
        ],
        "recommendations": ["re-run light rain"],
        "author": "claude-test",
        "cost_usd": 0.5,
    })
    assert analysis.verdict == "fail" and analysis.findings[2].severity == "info"
    assert ReportAnalysis.from_dict(analysis.to_dict()) == analysis
    html = _report().html(analysis=analysis, views=False)
    assert "Heavy rain &lt;fails&gt;" in html and "<fails>" not in html
    assert html.index("heavy rain") < html.index("light rain flaky")  # most severe first
    assert "re-run light rain" in html and "claude-test · $0.50" in html
    assert ReportAnalysis.from_dict({"verdict": "?"}).verdict == "inconclusive"


def test_the_cli(tmp_path: Path) -> None:
    runs = tmp_path / "runs" / "nightly"
    runs.mkdir(parents=True)
    for i, doc in enumerate(_campaign()):
        (runs / f"r{i:02d}_coverage.json").write_text(json.dumps(doc), encoding="utf-8")
    analysis = tmp_path / "analysis.json"
    analysis.write_text(json.dumps({"verdict": "pass", "headline": "fine", "summary": "ok",
                                    "findings": [], "recommendations": []}), encoding="utf-8")
    out = tmp_path / "out" / "report.html"
    summary = tmp_path / "summary.json"
    assert main([str(tmp_path / "runs"), "-o", str(out), "--analysis", str(analysis),
                 "--summary-json", str(summary), "--title", "CLI"]) == 0
    text = out.read_text(encoding="utf-8")
    assert "<title>CLI</title>" in text and "fine" in text
    assert "r01_coverage.json" in text and str(tmp_path) not in text  # names relative to the runs
    assert json.loads(summary.read_text(encoding="utf-8"))["runs"] == 13
    assert main([str(tmp_path / "runs"), "-o", str(out), "--no-agent", "--no-views"]) == 0
    assert "No analysis was made" in out.read_text(encoding="utf-8")
    assert main([str(tmp_path / "nothing"), "-o", str(out), "--no-agent"]) == 1


def test_it_is_exported() -> None:
    assert openodd.TestReport is TestReport and openodd.agent.DEFAULT_MODEL


# -- the agent, with the Claude Agent SDK's transport replaced ----------------------


sdk = pytest.importorskip("claude_agent_sdk")


def _call(tool: Any, args: dict[str, Any]) -> Any:
    out = asyncio.run(tool.handler(args))
    text = out["content"][0]["text"]
    return out.get("is_error", False), text


def test_the_tools_read_the_report() -> None:
    report = _report()
    tools = {t.name: t for t in openodd.agent.report_tools(report)}
    assert set(tools) == {"summary", "markdown_report", "failed_runs", "run", "coverage_item",
                          "odd_matrix", "drift"}
    err, text = _call(tools["summary"], {})
    assert not err and json.loads(text) == json.loads(json.dumps(report.summary()))
    err, text = _call(tools["failed_runs"], {"scenario": "cut_in", "limit": 2})
    assert not err and len(json.loads(text)) == 2
    err, text = _call(tools["run"], {"document": 0})
    assert not err and "cells" not in json.loads(text)["odd"]
    assert _call(tools["run"], {"document": 999})[0]
    err, text = _call(tools["odd_matrix"], {"x": "speed", "y": "rain"})
    assert not err and json.loads(text)["x"]["name"] == "speed"
    err, text = _call(tools["odd_matrix"], {"x": "nope"})
    assert err and "speed" in text
    assert not _call(tools["drift"], {})[0]
    assert _call(tools["coverage_item"], {"name": "nope"})[0]
    assert "# Coverage report" in _call(tools["markdown_report"], {})[1]


def _result(**kwargs: Any) -> Any:
    fields = {"subtype": "success", "duration_ms": 1, "duration_api_ms": 1, "is_error": False,
              "num_turns": 2, "session_id": "s"}
    return sdk.ResultMessage(**{**fields, **kwargs})


def test_analyze_through_the_sdk(monkeypatch: pytest.MonkeyPatch) -> None:
    seen: dict[str, Any] = {}

    async def fake_query(*, prompt: str, options: Any) -> Any:
        seen["prompt"], seen["options"] = prompt, options
        yield sdk.AssistantMessage(content=[], model="claude-served")
        yield _result(total_cost_usd=0.12, structured_output={
            "verdict": "fail", "headline": "h", "summary": "s",
            "findings": [{"severity": "major", "title": "t", "detail": "d", "evidence": ["e"]}],
            "recommendations": ["r"]})

    monkeypatch.setattr(sdk, "query", fake_query)
    analysis = openodd.agent.analyze(_report(), language="Japanese", instructions="pass rate 95%")
    assert analysis.verdict == "fail" and analysis.findings[0].evidence == ["e"]
    assert analysis.author == "claude-served" and analysis.cost_usd == pytest.approx(0.12)
    options = seen["options"]
    assert options.tools == [] and options.model == openodd.agent.DEFAULT_MODEL
    assert sorted(options.allowed_tools) == sorted(
        f"mcp__openodd__{n}" for n in ("summary", "markdown_report", "failed_runs", "run",
                                       "coverage_item", "odd_matrix", "drift"))
    assert options.output_format["schema"] is openodd.agent.ANALYSIS_SCHEMA
    assert "Japanese" in seen["prompt"] and "pass rate 95%" in seen["prompt"]


def test_analyze_reads_json_from_the_text_and_reports_errors(monkeypatch: pytest.MonkeyPatch) -> None:
    results = iter([
        _result(result='Here: {"verdict": "pass", "headline": "ok", "summary": "s"}'),
        _result(result="no json"),
        _result(is_error=True, subtype="error_max_turns", errors=["too many turns"]),
    ])

    async def fake_query(*, prompt: str, options: Any) -> Any:
        yield next(results)

    monkeypatch.setattr(sdk, "query", fake_query)
    report = _report()
    assert openodd.agent.analyze(report).verdict == "pass"
    with pytest.raises(openodd.agent.AnalysisError, match="no analysis"):
        openodd.agent.analyze(report)
    with pytest.raises(openodd.agent.AnalysisError, match="too many turns"):
        openodd.agent.analyze(report)


def test_the_cli_keeps_going_without_an_analysis(monkeypatch: pytest.MonkeyPatch, tmp_path: Path,
                                                 capsys: pytest.CaptureFixture[str]) -> None:
    async def fake_query(*, prompt: str, options: Any) -> Any:
        raise sdk.CLINotFoundError("no claude")
        yield  # pragma: no cover

    monkeypatch.setattr(sdk, "query", fake_query)
    (tmp_path / "r_coverage.json").write_text(json.dumps(_campaign()[0]), encoding="utf-8")
    out = tmp_path / "report.html"
    assert main([str(tmp_path), "-o", str(out)]) == 0
    assert "no analysis" in capsys.readouterr().err and out.exists()
