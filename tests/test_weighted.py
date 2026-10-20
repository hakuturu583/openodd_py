"""The naturalistic failure rate of importance-sampled runs (outcome.weight)."""

from __future__ import annotations

import json
import logging
import math
from typing import Any

import pytest

import openodd
from openodd import CoverageCollector, WeightedOutcome, merge_coverage

Z95 = 1.959963984540054


def doc(passed: bool, weight: Any = None, scenario: str = "s", **outcome: Any) -> dict[str, Any]:
    out: dict[str, Any] = {"passed": passed, **outcome}
    if weight is not None:
        out["weight"] = weight
    return {"schema": openodd.COVERAGE_SCHEMA, "scenario": scenario, "items": [], "outcome": out}


def test_the_collector_writes_a_weight_only_when_given(caplog: pytest.LogCaptureFixture) -> None:
    collector = CoverageCollector([])
    collector.record_outcome(True)
    assert collector.to_dict("s")["outcome"] == {"passed": True}
    collector.record_outcome(False, weight=0.25)
    assert collector.to_dict("s")["outcome"] == {"passed": False, "weight": 0.25}
    with caplog.at_level(logging.WARNING):
        for bad in (-1.0, float("nan"), float("inf"), True, "0.5"):
            collector.record_outcome(False, weight=bad)  # type: ignore[arg-type]
            assert collector.to_dict("s")["outcome"] == {"passed": False}
    assert "is not a likelihood ratio" in caplog.text


def test_unweighted_runs_have_no_weighted_outcome() -> None:
    report = merge_coverage([doc(True), doc(False)])
    assert report.weighted_outcomes == []
    assert "weighted_outcomes" not in report.to_dict()
    assert "Weighted outcomes" not in report.to_markdown()


def test_the_rate_is_the_mean_of_the_weighted_failures() -> None:
    weights = [0.01, 2.0, None, 0.03]
    passed = [False, True, True, False]
    report = merge_coverage([doc(p, w) for p, w in zip(passed, weights)])
    (o,) = report.weighted_outcomes
    assert isinstance(o, WeightedOutcome)
    terms = [0.01, 0.0, 0.0, 0.03]
    mean = sum(terms) / 4
    sd = math.sqrt(sum((t - mean) ** 2 for t in terms) / 3)
    half = Z95 * sd / math.sqrt(4)
    assert (o.runs, o.runs_failed, o.documents) == (4, 2, [0, 1, 2, 3])
    assert o.failure_rate == pytest.approx(mean)
    assert (o.ci_low, o.ci_high) == (0.0, pytest.approx(mean + half))
    assert o.relative_half_width == pytest.approx(half / mean)
    assert o.self_normalised_rate == pytest.approx(0.04 / 3.04)
    assert o.effective_runs == pytest.approx(3.04**2 / (0.01**2 + 4 + 1 + 0.03**2))
    assert report.to_dict()["weighted_outcomes"][0]["failure_rate"] == pytest.approx(mean)
    assert "## Weighted outcomes (importance sampling)" in report.to_markdown()


def test_groups_are_per_scenario_and_environment() -> None:
    report = merge_coverage(
        [
            doc(True, 1.0, environment="x"),
            doc(False, 0.5, environment="y"),
            doc(True, scenario="other"),  # no weight in its group: not reported
        ]
    )
    keys = [(o.scenario, o.environment) for o in report.weighted_outcomes]
    assert keys == [("s", "x"), ("s", "y")]
    assert report.weighted_outcomes[0].relative_half_width is None  # nothing failed


def test_a_written_coverage_file_round_trips() -> None:
    collector = CoverageCollector([])
    collector.record_outcome(False, weight=1e-4)
    (o,) = merge_coverage([json.loads(json.dumps(collector.to_dict("s")))]).weighted_outcomes
    assert o.failure_rate == pytest.approx(1e-4) and o.runs == 1
