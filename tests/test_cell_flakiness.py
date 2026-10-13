"""Flakiness per ODD cell: outcomes of the runs that drove through each cell."""

from __future__ import annotations

import copy
import json
import logging
import math
import random
from pathlib import Path
from typing import Any, Optional

import pytest
from hypothesis import given, settings
from hypothesis import strategies as st

import openodd
from openodd import (
    ABOVE_RANGE,
    BELOW_RANGE,
    CELL_BAND,
    CELL_MIN_RUNS,
    COVERAGE_SCHEMA,
    MISSING,
    CellFlakiness,
    CellRow,
    CoverageCollector,
    OddAttribute,
    OddDefinition,
    merge_coverage,
    wilson_interval,
)
from openodd.report import main

Z95 = 1.959963984540054


def reference_wilson(failed: int, runs: int) -> tuple[float, float]:
    """The Wilson score interval, written out independently of the Rust core."""
    if runs == 0:
        return 0.0, 1.0
    p, n, z2 = failed / runs, runs, Z95 * Z95
    denom = 1 + z2 / n
    centre = (p + z2 / (2 * n)) / denom
    half = Z95 * math.sqrt(p * (1 - p) / n + z2 / (4 * n * n)) / denom
    low = 0.0 if failed == 0 else max(0.0, centre - half)
    high = 1.0 if failed == runs else min(1.0, centre + half)
    return low, high


def reference_status(runs: int, failed: int, min_runs: int, band: tuple[float, float]) -> str:
    if runs == 0 or runs < min_runs:
        return "too_few"
    if failed == 0:
        return "passing"
    if failed == runs:
        return "failing"
    low, high = reference_wilson(failed, runs)
    return "flaky" if band[0] < low and high < band[1] else "mixed"


ATTRS = ["speed", "rain"]


def doc(
    rows: list[list[Any]],
    *,
    passed: Optional[bool] = None,
    scenario: str = "s",
    attributes: Optional[list[str]] = None,
    failed_cell: Optional[list[str]] = None,
    failed_at: Optional[float] = None,
    environment: Optional[str] = None,
    outcome: Any = "build",
) -> dict[str, Any]:
    """A coverage document with only what per-cell flakiness reads."""
    if outcome == "build":
        outcome = None
        if passed is not None:
            outcome = {"passed": passed}
            if failed_at is not None:
                outcome["failed_at"] = failed_at
            if failed_cell is not None:
                outcome["failed_cell"] = failed_cell
            if environment is not None:
                outcome["environment"] = environment
    return {
        "schema": COVERAGE_SCHEMA,
        "scenario": scenario,
        "items": [],
        "odd": {"name": "o", "cells": {"attributes": attributes or ATTRS, "rows": rows}},
        "outcome": outcome,
    }


SLOW = ["[0, 30)", "none"]
FAST = ["[30, 60]", "heavy"]


def row(labels: list[str], ticks: int = 10, seconds: float = 0.5, meters: float = 5.0) -> list[Any]:
    return [labels, ticks, seconds, meters]


def only(report: openodd.CoverageReport) -> CellFlakiness:
    groups = report.cell_flakiness
    assert len(groups) == 1, groups
    return groups[0]


# ---------------------------------------------------------------------------
# Counts, rates, intervals
# ---------------------------------------------------------------------------


def test_counts_rates_and_intervals_are_exact() -> None:
    docs = [
        doc([row(SLOW), row(FAST, 4, 0.2, 2.0)], passed=True),
        doc([row(SLOW), row(FAST, 4, 0.2, 2.0)], passed=False, failed_cell=FAST),
        doc([row(SLOW)], passed=False),  # failed, but nobody knows where
        doc([row(SLOW)], passed=True),
        doc([row(SLOW)], passed=False, failed_cell=SLOW),
    ]
    g = only(merge_coverage(docs))
    assert (g.scenario, g.environment, g.attributes) == ("s", None, ATTRS)
    assert (g.runs, g.runs_failed, g.failures_located, g.documents) == (5, 3, 2, [0, 1, 2, 3, 4])
    slow, fast = g.cell(SLOW), g.cell(FAST)
    assert slow is not None and fast is not None
    assert (slow.runs_visited, slow.runs_failed, slow.fail_at_cell) == (5, 3, 1)
    assert slow.fail_rate == pytest.approx(0.6)
    assert (slow.ci_low, slow.ci_high) == pytest.approx(reference_wilson(3, 5), abs=1e-12)
    assert (slow.ci_low, slow.ci_high) == pytest.approx((0.2307, 0.8824), abs=1e-4)
    assert (slow.seconds, slow.meters) == (2.5, 25.0)
    assert (fast.runs_visited, fast.runs_failed, fast.fail_at_cell) == (2, 1, 1)
    assert (fast.seconds, fast.meters) == (0.4, 4.0)
    # 3 of 5 (rarer outcome at least 11.8%) is flakier than 1 of 2 (9.5%).
    assert [r.labels for r in g.rows] == [tuple(SLOW), tuple(FAST)]
    assert slow.status() == "flaky" and fast.status() == "too_few"
    assert g.flaky_cells() == [slow]


def test_to_dict_has_the_documented_shape() -> None:
    docs = [doc([row(SLOW)], passed=True), doc([row(SLOW)], passed=False, failed_cell=SLOW)]
    out = merge_coverage(docs).cell_flakiness[0].to_dict()
    low, high = reference_wilson(1, 2)
    assert out == {
        "scenario": "s",
        "environment": None,
        "attributes": ATTRS,
        "runs": 2,
        "runs_failed": 1,
        "failures_located": 1,
        "documents": [0, 1],
        "rows": [[SLOW, 2, 1, 0.5, pytest.approx(low), pytest.approx(high), 1, 1.0, 10.0]],
    }
    full = merge_coverage(docs).to_dict()
    assert full["cell_flakiness"] == [out]
    assert CellFlakiness.from_dict(json.loads(json.dumps(out))) == merge_coverage(docs).cell_flakiness[0]


def test_statuses_and_thresholds() -> None:
    always = [doc([row(SLOW)], passed=False, failed_cell=SLOW) for _ in range(4)]
    never = [doc([row(FAST)], passed=True) for _ in range(4)]
    g = only(merge_coverage(always + never))
    assert g.cell(SLOW).status() == "failing"  # a bug, not flakiness
    assert g.cell(FAST).status() == "passing"
    assert g.flaky_cells() == [] and g.failing_cells() == [g.cell(SLOW)]
    # The failing cell sorts before the passing one: same flakiness (0), higher rate.
    assert [r.labels for r in g.rows] == [tuple(SLOW), tuple(FAST)]
    assert g.cell(SLOW).fail_at_cell == 4

    # One failure in ten: both outcomes, but it may be a 2% rarity.
    one_in_ten = [doc([row(SLOW)], passed=i > 0) for i in range(10)]
    r = only(merge_coverage(one_in_ten)).cell(SLOW)
    assert r.status() == "mixed" and not r.is_flaky()
    assert r.is_flaky(band=(0.0, 1.0))  # any mix, when asked for
    assert r.status(min_runs=11) == "too_few"
    with pytest.raises(ValueError, match="band"):
        r.status(band=(0.5, 0.5))
    with pytest.raises(ValueError, match="min_runs"):
        r.status(min_runs=0)
    assert CELL_MIN_RUNS == 3 and CELL_BAND == (0.05, 0.95)


def test_report_flaky_cells_pairs_each_with_its_group() -> None:
    docs = [doc([row(SLOW)], passed=i % 2 == 0, environment="e") for i in range(6)]
    pairs = merge_coverage(docs).flaky_cells()
    assert [(g.environment, r.labels) for g, r in pairs] == [("e", tuple(SLOW))]


@given(st.integers(0, 60).flatmap(lambda n: st.tuples(st.integers(0, n), st.just(n))))
def test_the_interval_matches_the_reference(case: tuple[int, int]) -> None:
    failed, runs = case
    assert wilson_interval(failed, runs) == pytest.approx(reference_wilson(failed, runs), abs=1e-12)
    low, high = wilson_interval(failed, runs)
    assert 0.0 <= low <= (failed / runs if runs else 0.0) <= high <= 1.0 or runs == 0
    for min_runs in (1, 3, 10):
        for band in ((0.0, 1.0), (0.05, 0.95), (0.2, 0.6)):
            r = CellRow(("x",), runs, failed, failed / runs if runs else 0.0, low, high, 0, 0.0, 0.0)
            assert r.status(min_runs, band) == reference_status(runs, failed, min_runs, band)


def test_more_failures_than_runs_is_refused() -> None:
    with pytest.raises(ValueError):
        wilson_interval(3, 2)


# ---------------------------------------------------------------------------
# Edge cases
# ---------------------------------------------------------------------------


def test_a_cell_no_run_visited_is_not_listed() -> None:
    g = only(merge_coverage([
        doc([row(SLOW), row(FAST, ticks=0, seconds=0.0, meters=0.0)], passed=False, failed_cell=FAST),
    ]))
    assert g.cell(FAST) is None  # a row of no steps is no visit
    assert [r.labels for r in g.rows] == [tuple(SLOW)]
    assert g.failures_located == 0  # a failure in a cell it never visited is not located


def test_a_run_of_no_steps_counts_but_visits_nothing() -> None:
    g = only(merge_coverage([doc([], passed=False), doc([row(SLOW)], passed=True)]))
    assert (g.runs, g.runs_failed) == (2, 1)
    assert g.cell(SLOW).runs_visited == 1 and g.cell(SLOW).runs_failed == 0


def test_missing_and_out_of_range_labels_are_cells_of_their_own() -> None:
    labels = [[MISSING, "none"], [BELOW_RANGE, "heavy"], [ABOVE_RANGE, MISSING]]
    docs = [doc([row(lab) for lab in labels], passed=False, failed_cell=[ABOVE_RANGE, MISSING]),
            doc([row(labels[0])], passed=True)]
    g = only(merge_coverage(docs))
    assert {r.labels for r in g.rows} == {tuple(lab) for lab in labels}
    assert g.cell([ABOVE_RANGE, MISSING]).fail_at_cell == 1
    assert g.cell([MISSING, "none"]).runs_visited == 2


def test_environments_are_never_pooled() -> None:
    docs = [
        doc([row(SLOW)], passed=True, environment="sha256:aaa"),
        doc([row(SLOW)], passed=False, environment="sha256:bbb"),
        doc([row(SLOW)], passed=False),
        doc([row(SLOW)], passed=True),
        doc([row(SLOW)], passed=True, environment="sha256:aaa", scenario="other"),
    ]
    groups = merge_coverage(docs).cell_flakiness
    assert [(g.scenario, g.environment, g.documents) for g in groups] == [
        ("s", "sha256:aaa", [0]),
        ("s", "sha256:bbb", [1]),
        ("s", None, [2, 3]),
        ("other", "sha256:aaa", [4]),
    ]
    assert groups[0].cell(SLOW).runs_failed == 0 and groups[1].cell(SLOW).runs_failed == 1


def test_different_attribute_lists_are_reported_apart_with_a_warning(
    caplog: pytest.LogCaptureFixture,
) -> None:
    docs = [
        doc([row(SLOW)], passed=True),
        doc([[["[0, 30)"], 3, 0.1, 1.0]], passed=False, attributes=["speed"]),
        doc([row(SLOW)], passed=False),
    ]
    with caplog.at_level(logging.WARNING, logger="openodd.report"):
        report = merge_coverage(docs)
    groups = report.cell_flakiness
    assert [(g.attributes, g.documents) for g in groups] == [(ATTRS, [0, 2]), (["speed"], [1])]
    assert "measured different ODD attributes" in caplog.text
    assert "documents [0, 2]" in caplog.text and "documents [1]" in caplog.text
    assert "measured different ODD attributes" in report.to_markdown()


def test_a_row_of_the_wrong_length_is_skipped() -> None:
    g = only(merge_coverage([doc([row(SLOW), [["only-one"], 2, 0.1, 0.0]], passed=True)]))
    assert [r.labels for r in g.rows] == [tuple(SLOW)]


# ---------------------------------------------------------------------------
# Old files
# ---------------------------------------------------------------------------


def test_older_files_merge_as_before() -> None:
    # Before outcomes (no key, or null), and before cells: nothing new anywhere.
    before = [
        doc([row(SLOW)], outcome=None),
        {k: v for k, v in doc([row(SLOW)]).items() if k != "outcome"},
        {"schema": "autoware_carla_scenario.coverage/1", "scenario": "s", "items": []},
        {"schema": COVERAGE_SCHEMA, "scenario": "s", "items": [], "odd": None,
         "outcome": {"passed": False}},  # an outcome, but no cells
    ]
    report = merge_coverage(before)
    assert report.runs == 4 and report.cell_flakiness == []
    assert "cell_flakiness" not in report.to_dict()
    assert "Flakiness per ODD cell" not in report.to_markdown()


def test_v060_outcomes_count_without_a_location() -> None:
    docs = [doc([row(SLOW)], outcome={"passed": False}), doc([row(SLOW)], outcome={"passed": True})]
    g = only(merge_coverage(docs))
    assert (g.runs, g.runs_failed, g.failures_located) == (2, 1, 0)
    assert g.cell(SLOW).fail_at_cell == 0
    # Everything the report had before is unchanged by the new section.
    stripped = merge_coverage([{k: v for k, v in d.items() if k != "outcome"} for d in docs]).to_dict()
    with_outcomes = merge_coverage(docs).to_dict()
    assert with_outcomes.pop("cell_flakiness")
    assert with_outcomes == stripped


# ---------------------------------------------------------------------------
# The collector
# ---------------------------------------------------------------------------


def _odd() -> OddDefinition:
    speed = OddAttribute("speed", lambda c: c["speed"], unit="km/h", buckets=[0, 30, 60])
    rain = OddAttribute("rain", lambda c: c["rain"], values=["none", "heavy"])
    return OddDefinition("cells", [speed, rain])


def _drive(collector: CoverageCollector, steps: list[tuple[float, Any, Any]]) -> None:
    collector.start({"speed": None, "rain": None}, 0.0)
    for t, speed, rain in steps:
        collector.tick({"speed": speed, "rain": rain}, t)
    collector.end({"speed": None, "rain": None}, steps[-1][0] if steps else 0.0)


STEPS = [(0.1, 10.0, "none"), (0.2, 40.0, "heavy"), (0.3, 70.0, "heavy"), (0.4, None, "none")]


@pytest.mark.parametrize(
    ("failed_at", "cell"),
    [
        (0.05, None),  # before the first sample: nowhere
        (0.1, ["[0, 30)", "none"]),  # at a sample: that one
        (0.25, ["[30, 60]", "heavy"]),  # between: the one before
        (0.3, [ABOVE_RANGE, "heavy"]),
        (9.0, [MISSING, "none"]),  # after the last: the last
    ],
)
def test_the_collector_records_where_the_run_failed(failed_at: float, cell: Optional[list[str]]) -> None:
    collector = CoverageCollector([], odd=_odd())
    _drive(collector, STEPS)
    collector.record_outcome(False, failed_at=failed_at, environment="env-1")
    outcome = collector.to_dict("s")["outcome"]
    expected: dict[str, Any] = {"passed": False, "failed_at": failed_at, "environment": "env-1"}
    if cell is not None:
        expected["failed_cell"] = cell
    assert outcome == expected
    g = only(merge_coverage([json.loads(json.dumps(collector.to_dict("s")))]))
    assert g.environment == "env-1" and g.failures_located == (cell is not None)
    if cell is not None:
        assert g.cell(cell).fail_at_cell == 1
        assert sum(r.fail_at_cell for r in g.rows) == 1


def test_the_collector_writes_only_what_was_given(caplog: pytest.LogCaptureFixture) -> None:
    collector = CoverageCollector([], odd=_odd())
    _drive(collector, STEPS)
    collector.record_outcome(True)
    assert collector.to_dict("s")["outcome"] == {"passed": True}
    with caplog.at_level(logging.WARNING):
        collector.record_outcome(True, failed_at=0.2)
    assert collector.to_dict("s")["outcome"] == {"passed": True}
    assert "of a run that passed is ignored" in caplog.text
    collector.record_outcome(False, failed_at=float("nan"))
    assert collector.to_dict("s")["outcome"] == {"passed": False}
    collector.record_outcome(False, environment="e")
    assert collector.to_dict("s")["outcome"] == {"passed": False, "environment": "e"}
    # Without an ODD, the time is kept and no cell is.
    plain = CoverageCollector([])
    plain.record_outcome(False, failed_at=1.23456)
    assert plain.to_dict("s")["outcome"] == {"passed": False, "failed_at": 1.235}
    # A run of no steps: no sample to place the failure in.
    empty = CoverageCollector([], odd=_odd())
    empty.record_outcome(False, failed_at=0.0)
    assert empty.to_dict("s")["outcome"] == {"passed": False, "failed_at": 0.0}


def test_cells_add_returns_the_cell() -> None:
    cells = openodd.OddCells(_odd())
    assert cells.add({"speed": 45.0, "rain": "heavy"}, 0.1) == ["[30, 60]", "heavy"]


# ---------------------------------------------------------------------------
# Markdown and the command line
# ---------------------------------------------------------------------------


def test_markdown_has_its_own_row_limit() -> None:
    labels = [[f"[{i}, {i + 1})", "none"] for i in range(25)]
    docs = [doc([row(lab) for lab in labels], passed=i % 2 == 0, failed_cell=None if i % 2 == 0 else labels[0])
            for i in range(4)]
    markdown = merge_coverage(docs).to_markdown(max_holes=0)
    section = markdown.split("## Flakiness per ODD cell", 1)[1]
    assert "### s (no environment)" in section
    assert "| speed x rain | Status | Runs | Failed | Fail rate | 95% interval | Failed here |" in section
    shown = [line for line in section.splitlines() if line.startswith("| [")]
    assert len(shown) == 20
    assert "... (5 more)" in section
    assert "| [0, 1) x none | flaky | 4 | 2 | 50% | 15%-85% | 2 | 2.0 | 20 |" in section
    assert "Runs: 4 (2 failed, 2 of them located in a cell)" in section


def test_the_cli_reports_the_cells(tmp_path: Path, capsys: pytest.CaptureFixture[str]) -> None:
    for i in range(4):
        d = doc([row(SLOW), row(FAST)], passed=i < 2, failed_cell=None if i < 2 else FAST,
                environment="abc")
        (tmp_path / f"run{i}_coverage.json").write_text(json.dumps(d), encoding="utf-8")
    out = tmp_path / "report.json"
    assert main([str(tmp_path), "--json", str(out)]) == 0
    printed = capsys.readouterr().out
    assert "## Flakiness per ODD cell" in printed and "### s (abc)" in printed
    groups = json.loads(out.read_text())["cell_flakiness"]
    assert groups[0]["environment"] == "abc" and groups[0]["failures_located"] == 2


# ---------------------------------------------------------------------------
# Rust/Python parity: the merge against a reference written here
# ---------------------------------------------------------------------------


def reference_cells(documents: list[dict[str, Any]]) -> list[dict[str, Any]]:
    groups: dict[tuple[Any, ...], dict[str, Any]] = {}
    for index, d in enumerate(documents):
        outcome = d.get("outcome")
        cells = (d.get("odd") or {}).get("cells")
        if not isinstance(outcome, dict) or not isinstance(outcome.get("passed"), bool) or not cells:
            continue
        attrs = cells["attributes"]
        visited: dict[tuple[str, ...], list[float]] = {}
        for labels, ticks, seconds, meters in cells["rows"]:
            if len(labels) == len(attrs) and ticks > 0:
                slot = visited.setdefault(tuple(labels), [0.0, 0.0])
                slot[0] += max(seconds, 0.0)
                slot[1] += max(meters, 0.0)
        passed = outcome["passed"]
        at = outcome.get("failed_cell")
        at = tuple(at) if not passed and at is not None and tuple(at) in visited else None
        key = (d["scenario"], outcome.get("environment"), tuple(attrs))
        g = groups.setdefault(key, {"documents": [], "failed": 0, "located": 0, "rows": {}})
        g["documents"].append(index)
        g["failed"] += not passed
        g["located"] += at is not None
        for labels, (seconds, meters) in visited.items():
            r = g["rows"].setdefault(labels, [0, 0, 0, 0.0, 0.0])
            r[0] += 1
            r[1] += not passed
            r[2] += at == labels
            r[3] += seconds
            r[4] += meters
    out = []
    for (scenario, env, attrs), g in groups.items():
        rows = []
        for labels, (n, k, at, s, m) in g["rows"].items():
            low, high = reference_wilson(k, n)
            rows.append([list(labels), n, k, k / n, low, high, at, round(s, 3), round(m, 3)])
        rows.sort(key=lambda r: (-min(r[4], 1 - r[5]), -r[3], r[0]))
        out.append({"scenario": scenario, "environment": env, "attributes": list(attrs),
                    "runs": len(g["documents"]), "runs_failed": g["failed"],
                    "failures_located": g["located"], "documents": g["documents"], "rows": rows})
    return out


def _random_documents(seed: int) -> list[dict[str, Any]]:
    rng = random.Random(seed)
    speeds = ["[0, 30)", "[30, 60]", BELOW_RANGE, ABOVE_RANGE, MISSING]
    rains = ["none", "heavy", MISSING]
    docs = []
    for _ in range(rng.randint(0, 25)):
        attrs = ATTRS if rng.random() < 0.9 else ["speed"]
        pool = [[s, r] for s in speeds for r in rains] if len(attrs) == 2 else [[s] for s in speeds]
        rows = [[lab, rng.choice([0, 1, 5, 20]), round(rng.uniform(0, 3), 3), round(rng.uniform(0, 30), 3)]
                for lab in rng.sample(pool, rng.randint(0, 5))]
        passed = rng.random() < 0.6
        visited = [r[0] for r in rows]
        failed_cell = rng.choice(visited + [None, ["nowhere"] * len(attrs)]) if visited else None
        d = doc(rows, passed=passed, scenario=rng.choice(["a", "b"]), attributes=attrs,
                failed_cell=None if passed else failed_cell,
                environment=rng.choice([None, "e1", "e2"]))
        if rng.random() < 0.1:
            d["outcome"] = rng.choice([None, {"passed": None}, {}])
        docs.append(d)
    return docs


@settings(max_examples=150, deadline=None)
@given(st.integers(0, 10**6))
def test_the_merge_matches_the_reference(seed: int) -> None:
    docs = _random_documents(seed)
    got = [g.to_dict() for g in merge_coverage(copy.deepcopy(docs)).cell_flakiness]
    want = reference_cells(docs)
    assert len(got) == len(want)
    for g, w in zip(got, want):
        rows_g, rows_w = g.pop("rows"), w.pop("rows")
        assert g == w
        assert [r[0] for r in rows_g] == [r[0] for r in rows_w]
        for a, b in zip(rows_g, rows_w):
            assert a[1:3] == b[1:3] and a[6] == b[6]
            assert a[3:6] == pytest.approx(b[3:6], abs=1e-12)
            assert a[7:] == pytest.approx(b[7:], abs=1e-9)
