"""The coverage collector: sampling on events, exposure, criteria, the ODD."""

from __future__ import annotations

import json
from pathlib import Path
from types import SimpleNamespace
from typing import Any, Optional

import pytest

from openodd import (
    ABOVE_RANGE,
    BELOW_RANGE,
    COVERAGE_SCHEMA,
    Backend,
    CoverageCollector,
    CoverItem,
    CrossItem,
    OddAttribute,
    OddDefinition,
    OddModule,
    SamplingEvent,
    merge_coverage,
)


def _item(name: str = "x", **kw: Any) -> CoverItem:
    kw.setdefault("expression", lambda context: context.value)
    return CoverItem(name=name, **kw)


class _Context:
    """Just enough of a context for expressions that read ``context.value``."""

    def __init__(self, value: Any = None) -> None:
        self.value = value


class _Flag:
    """An event of the backend's own: satisfied while ``context.flag`` is true."""

    label = "flag"

    def check(self, context: Any, elapsed: float) -> Optional[bool]:
        return True if getattr(context, "flag", False) else None


class _Positioned(Backend):
    """A backend that knows where the ego is: ``context.x`` along the x axis."""

    def position(self, context: Any) -> Optional[tuple[float, float, float]]:
        x = getattr(context, "x", None)
        return None if x is None else (x, 0.0, 0.0)


class TestCollector:
    def test_an_end_item_is_sampled_once_at_the_end(self) -> None:
        collector = CoverageCollector([_item(values=[1, 2])])
        context = _Context(1)
        collector.start(context, 0.0)
        for _ in range(3):
            collector.tick(context, 0.0)
        context.value = 2
        collector.end(context, 1.0)
        assert collector.to_dict("S")["items"][0]["hits"] == {"1": 0, "2": 1}

    def test_a_tick_item_counts_every_tick(self) -> None:
        collector = CoverageCollector([_item(values=[1, 2], event=SamplingEvent.TICK)])
        context = _Context(1)
        for _ in range(3):
            collector.tick(context, 0.0)
        collector.end(context, 0.0)
        assert collector.to_dict("S")["items"][0]["hits"] == {"1": 3, "2": 0}

    def test_an_event_of_the_backend_samples_when_it_becomes_satisfied(self) -> None:
        collector = CoverageCollector([_item(values=[1, 2, 3], event=_Flag())])
        context = SimpleNamespace(value=1, flag=False)
        for value, flag in [(1, False), (2, True), (3, True), (3, False), (3, True)]:
            context.value, context.flag = value, flag
            collector.tick(context, 0.0)
        item = collector.to_dict("S")["items"][0]
        assert item["hits"] == {"1": 0, "2": 1, "3": 1}
        assert item["event"] == "condition:flag"

    def test_none_and_a_raising_expression_take_no_sample(self) -> None:
        def boom(context: Any) -> float:
            raise RuntimeError("weather unsupported")

        collector = CoverageCollector(
            [
                _item("none", values=[1], event=SamplingEvent.TICK),
                CoverItem("boom", boom, values=[1], event=SamplingEvent.TICK),
            ]
        )
        for _ in range(2):
            collector.tick(_Context(None), 0.0)
        items = collector.to_dict("S")["items"]
        assert [i["samples"] for i in items] == [0, 0]

    def test_ignored_and_out_of_range_samples_are_counted_apart(self) -> None:
        collector = CoverageCollector(
            [
                _item("n", buckets=[0, 10], ignore=lambda v: v == 5, event=SamplingEvent.TICK),
                _item("c", values=[1], event=SamplingEvent.TICK),
            ]
        )
        for value in (1, 5, 50, -3):
            collector.tick(_Context(value), 0.0)
        n, c = collector.to_dict("S")["items"]
        assert n["hits"] == {"[0, 10]": 1}
        assert n["ignored"] == 1
        assert n["out_of_range"] == {ABOVE_RANGE: 1, BELOW_RANGE: 1}
        assert c["out_of_range"] == {"5": 1, "50": 1, "-3": 1}

    def test_nan_is_no_sample(self) -> None:
        collector = CoverageCollector([_item(buckets=[0, 10], event=SamplingEvent.TICK)])
        collector.tick(_Context(float("nan")), 0.0)
        assert collector.to_dict("S")["items"][0]["samples"] == 0

    def test_a_cross_counts_the_buckets_hit_together(self) -> None:
        a = CoverItem("a", lambda c: c.value[0], values=[1, 2], event=SamplingEvent.TICK)
        b = CoverItem("b", lambda c: c.value[1], values=["x", "y"], event=SamplingEvent.TICK)
        collector = CoverageCollector([a, b], [CrossItem("ab", [a, b])])
        for value in [(1, "x"), (1, "x"), (2, "y"), (2, "z")]:
            collector.tick(_Context(value), 0.0)
        cross = collector.to_dict("S")["crosses"][0]
        assert cross["hits"] == {"1 / x": 2, "1 / y": 0, "2 / x": 0, "2 / y": 1}

    def test_names_must_be_unique(self) -> None:
        with pytest.raises(ValueError, match="more than once"):
            CoverageCollector([_item("a", values=[1]), _item("a", values=[2])])

    def test_the_file_is_written_in_the_coverage_schema(self, tmp_path: Path) -> None:
        collector = CoverageCollector([_item(values=[1])])
        path = tmp_path / "out" / "S_coverage.json"
        collector.write(path, "S")
        doc = json.loads(path.read_text())
        assert doc["schema"] == COVERAGE_SCHEMA
        assert doc["scenario"] == "S"
        assert merge_coverage([doc]).runs == 1


class TestExposure:
    @pytest.fixture
    def drive(self) -> Any:
        """Run a tick item over ``(value, x, elapsed)`` steps; the ego is at ``x``."""

        def run(steps: list[tuple[Any, Optional[float], float]], **kw: Any) -> dict[str, Any]:
            item = _item(values=["a", "b"], event=SamplingEvent.TICK, **kw)
            collector = CoverageCollector([item], backend=_Positioned())
            context = SimpleNamespace(value=None, x=0.0)
            collector.start(context, 0.0)
            for value, x, elapsed in steps:
                context.value, context.x = value, x
                collector.tick(context, elapsed)
            collector.end(context, steps[-1][2])
            return collector.to_dict("s")["items"][0]

        return run

    def test_a_tick_counts_the_step_since_the_previous_one(self, drive: Any) -> None:
        out = drive([("a", 10.0, 1.0), ("a", 30.0, 2.0), ("b", 35.0, 2.5)])
        assert out["hits"] == {"a": 2, "b": 1}
        assert out["seconds"] == {"a": 2.0, "b": 0.5}
        assert out["meters"] == {"a": 30.0, "b": 5.0}
        assert out["entries"] == {"a": 1, "b": 1}

    def test_leaving_and_coming_back_is_a_new_entry(self, drive: Any) -> None:
        out = drive(
            [("a", 1.0, 1.0), ("b", 2.0, 2.0), ("a", 3.0, 3.0), (None, 4.0, 4.0), ("a", 5.0, 5.0)]
        )
        assert out["entries"] == {"a": 3, "b": 1}

    def test_a_teleport_adds_no_distance(self, drive: Any) -> None:
        out = drive([("a", 1.0, 1.0), ("a", 5000.0, 1.05)])
        assert out["meters"]["a"] == 1.0

    def test_no_ego_means_no_distance(self, drive: Any) -> None:
        out = drive([("a", None, 1.0), ("a", None, 2.0)])
        assert out["meters"]["a"] == 0.0
        assert out["seconds"]["a"] == 2.0

    def test_a_nan_position_adds_no_distance(self, drive: Any) -> None:
        out = drive([("a", 1.0, 1.0), ("a", float("nan"), 2.0), ("a", 3.0, 3.0)])
        assert out["meters"]["a"] == 1.0

    def test_a_clock_going_back_counts_no_time_twice(self, drive: Any) -> None:
        out = drive([("a", 0.0, 2.0), ("a", 0.0, 1.5), ("a", 0.0, 3.0)])
        assert out["seconds"]["a"] == 3.0

    def test_the_default_backend_knows_no_position(self) -> None:
        collector = CoverageCollector([_item(values=["a"], event=SamplingEvent.TICK)])
        collector.start(_Context("a"), 0.0)
        collector.tick(_Context("a"), 1.0)
        out = collector.to_dict("s")["items"][0]
        assert out["seconds"]["a"] == 1.0
        assert out["meters"]["a"] == 0.0

    def test_without_tick_items_the_ego_is_not_looked_up(self) -> None:
        calls: list[Any] = []

        class Counting(Backend):
            def position(self, context: Any) -> Optional[tuple[float, float, float]]:
                calls.append(context)
                return None

        collector = CoverageCollector([_item(values=["a"])], backend=Counting())
        collector.start(_Context(), 0.0)
        for t in range(1, 5):
            collector.tick(_Context(), float(t))
        collector.end(_Context("a"), 4.0)
        assert calls == []

    def test_a_one_shot_sample_is_an_entry_of_no_duration(self) -> None:
        collector = CoverageCollector([_item(values=["a", "b"])])
        collector.start(_Context(), 0.0)
        collector.tick(_Context(), 5.0)
        collector.end(_Context("a"), 5.0)
        out = collector.to_dict("s")["items"][0]
        assert out["entries"] == {"a": 1, "b": 0}
        assert out["seconds"] == {"a": 0.0, "b": 0.0}

    def test_a_cross_cell_has_exposure_too(self) -> None:
        x = _item("x", values=["a", "b"], event=SamplingEvent.TICK)
        y = _item("y", values=["c"], event=SamplingEvent.TICK, expression=lambda c: "c")
        collector = CoverageCollector([x, y], [CrossItem("xy", [x, y])], backend=_Positioned())
        context = SimpleNamespace(value="a", x=0.0)
        collector.start(context, 0.0)
        context.x = 4.0
        collector.tick(context, 1.0)
        cross = collector.to_dict("s")["crosses"][0]
        assert cross["meters"]["a / c"] == 4.0
        assert cross["entries"]["a / c"] == 1


class TestCriteria:
    @pytest.fixture
    def drive(self) -> Any:
        def run(steps: list[tuple[Any, float, float]], **kw: Any) -> dict[str, Any]:
            item = _item(values=["a", "b"], event=SamplingEvent.TICK, **kw)
            collector = CoverageCollector([item], backend=_Positioned())
            context = SimpleNamespace(value=None, x=0.0)
            collector.start(context, 0.0)
            for value, x, elapsed in steps:
                context.value, context.x = value, x
                collector.tick(context, elapsed)
            collector.end(context, steps[-1][2])
            return collector.to_dict("s")

        return run

    def test_a_bucket_can_be_covered_by_distance(self, drive: Any) -> None:
        doc = drive(
            [("a", 30.0, 1.0), ("a", 60.0, 2.0), ("b", 70.0, 3.0)], cover_by="meters", target=50
        )
        entry = merge_coverage([doc]).entries[0]
        assert entry.covered == ["a"]
        assert entry.holes == ["b"]

    def test_a_short_stay_does_not_count(self, drive: Any) -> None:
        # "b" is clipped for 0.5 s between two long stays in "a".
        doc = drive(
            [("a", 10.0, 1.0), ("a", 20.0, 2.0), ("b", 25.0, 2.5), ("a", 35.0, 3.5), ("a", 45.0, 4.5)],
            cover_by="entries",
            min_stay=1.0,
        )
        raw = doc["items"][0]
        assert raw["entries"] == {"a": 2, "b": 1}
        assert raw["counted"]["entries"] == {"a": 2, "b": 0}
        assert raw["counted"]["meters"] == {"a": 40.0, "b": 0}
        assert merge_coverage([doc]).entries[0].holes == ["b"]

    def test_the_last_stay_counts_when_long_enough(self, drive: Any) -> None:
        doc = drive([("b", 0.0, 1.0), ("b", 0.0, 3.0)], cover_by="seconds", target=3, min_stay=2.0)
        assert doc["items"][0]["counted"]["seconds"]["b"] == 3.0
        assert merge_coverage([doc]).entries[0].covered == ["b"]

    def test_runs_sum_the_counted_amounts(self, drive: Any) -> None:
        doc = drive([("a", 30.0, 1.0)], cover_by="meters", target=50, min_stay=0.5)
        report = merge_coverage([doc, doc])
        assert report.entries[0].amount("a") == 60.0
        assert report.entries[0].covered == ["a"]


def _speed_odd() -> OddDefinition:
    speed = OddAttribute("speed", lambda c: c.speed, unit="km/h", buckets=[0, 30, 60, 90])
    hidden = OddAttribute("hidden", lambda c: c.hidden)
    fast = OddModule("fast", include_and=[speed.at_least(30)], situation=True)
    return OddDefinition(
        "speed_odd",
        [speed, hidden],
        [OddModule("slow", include_and=[speed.less_than(60)]), fast],
        roots=["slow"],
    )


class TestOdd:
    def test_every_tick_is_judged_and_fills_the_odd_items(self) -> None:
        collector = CoverageCollector([], odd=_speed_odd())
        collector.start(SimpleNamespace(speed=0, hidden=1), 0.0)
        for t, speed in enumerate([10, 40, 70, None], start=1):
            collector.tick(SimpleNamespace(speed=speed, hidden=1), float(t))
        doc = collector.to_dict("S")
        speed, fast = doc["items"]
        assert speed["name"] == "odd.speed"
        assert speed["hits"] == {"[0, 30)": 1, "[30, 60)": 1, "[60, 90]": 1}
        assert speed["outside_odd"] == ["[60, 90]"]
        assert fast["name"] == "odd.situation.fast"
        assert fast["hits"] == {"holds": 2}
        odd = doc["odd"]
        assert odd["ticks"] == {"inside": 2, "assumed": 1, "outside": 1}
        assert odd["module_ticks"] == {"slow": {"failed_ticks": 1, "missing_ticks": 1}}
        assert odd["out_intervals"] == [[2.0, 3.0]]
        assert [r[3:] for r in odd["samples"]["rows"]] == [[10, 1], [40, 1], [70, 1], [None, 1]]
        report = merge_coverage([doc])
        assert report.odds["speed_odd"].runs_outside == 1

    def test_the_backend_gives_the_samples_their_place(self) -> None:
        class Located(Backend):
            def geolocation(self, context: Any) -> Optional[tuple[float, float]]:
                return (35.6, 139.7)

        collector = CoverageCollector([], odd=_speed_odd(), backend=Located())
        collector.tick(SimpleNamespace(speed=10, hidden=1), 0.5)
        rows = collector.to_dict("S")["odd"]["samples"]["rows"]
        assert rows == [[0.5, 35.6, 139.7, 10, 1]]

    def test_each_tick_is_counted_in_its_cell(self) -> None:
        collector = CoverageCollector([], odd=_speed_odd(), backend=_Positioned())
        collector.start(SimpleNamespace(speed=0, hidden=1, x=0.0), 0.0)
        for t, (speed, x) in enumerate([(10, 5.0), (10, 10.0), (40, 20.0)], start=1):
            collector.tick(SimpleNamespace(speed=speed, hidden=1, x=x), float(t))
        cells = collector.to_dict("S")["odd"]["cells"]
        assert cells["attributes"] == ["speed"]
        assert cells["rows"] == [[["[0, 30)"], 2, 2.0, 10.0], [["[30, 60)"], 1, 1.0, 10.0]]

    def test_the_file_says_what_wrote_it(self) -> None:
        import openodd

        collector = CoverageCollector([], generator={"framework": "app==1.0"})
        assert collector.to_dict("S")["generator"] == {
            "openodd": openodd.__version__,
            "framework": "app==1.0",
        }

    def test_an_odd_read_from_files_is_archived_beside_the_coverage(self, tmp_path: Path) -> None:
        import openodd

        odd = openodd.load_openodd(
            "TAXONOMY:\n  speed: float velocity\n", name="o",
            bindings={"speed": {"probe": "pkg:none", "buckets": [0, 50]}},
            backend=Backend(probes={"pkg:none": (lambda c: c.speed, "km/h")}, allow_imports=False),
        )
        collector = CoverageCollector([], odd=odd)
        collector.tick(SimpleNamespace(speed=10), 0.1)
        collector.write(tmp_path / "S_coverage.json", "S")
        archive = (tmp_path / "S_odd.tar.gz").read_bytes()
        assert openodd.load_odd_archive(archive).revision == odd.revision

    def test_an_odd_built_in_code_has_no_archive(self, tmp_path: Path) -> None:
        collector = CoverageCollector([], odd=_speed_odd())
        collector.write(tmp_path / "S_coverage.json", "S")
        assert not (tmp_path / "S_odd.tar.gz").exists()
