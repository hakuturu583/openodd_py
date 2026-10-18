"""ODDs written in Python, registered by name, and resolve_odd()."""

from __future__ import annotations

import sys
import types
from pathlib import Path

import openodd
import pytest
from openodd import OddAttribute, OddDefinition, OddModule, registry


def _speed(context: dict) -> float | None:
    return context.get("speed")


def small_odd() -> OddDefinition:
    speed = OddAttribute("speed", _speed, unit="km/h", buckets=[0, 30, 60])
    return OddDefinition(
        "small", [speed], [OddModule("slow", include_and=[speed.between(0, 30)])]
    )


@pytest.fixture(autouse=True)
def _clean_registry(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(registry, "_REGISTRY", {})
    monkeypatch.setattr(registry, "_GROUPS", [registry.ENTRY_POINT_GROUP])
    monkeypatch.setattr(registry, "_LOADED_GROUPS", set())


def test_register_and_resolve_by_name() -> None:
    openodd.register_odd("small", small_odd)
    assert openodd.odd_builder("small") is small_odd
    assert "small" in openodd.odd_names()
    assert openodd.odd_names()[0] == openodd.DEFAULT_ODD
    odd = openodd.resolve_odd("small")
    assert odd.name == "small"
    assert odd.evaluate(odd.sample({"speed": 20.0})).inside
    assert not odd.evaluate(odd.sample({"speed": 50.0})).inside
    assert openodd.unregister_odd("small") is small_odd
    with pytest.raises(ValueError, match="no ODD named"):
        openodd.resolve_odd("small")


def test_register_rejects_bundled_and_empty_names() -> None:
    with pytest.raises(ValueError, match="bundled"):
        openodd.register_odd("default", small_odd)
    with pytest.raises(ValueError, match="empty"):
        openodd.register_odd("", small_odd)


def _bundled_backend() -> openodd.Backend:
    names = ["in_junction", "lanelet_location", "lanelet_subtype", "speed_limit_kph", "lane_count",
             "illumination", "rain", "fog", "ego_speed_kph", "traffic_density", "pedestrian_nearby"]
    units = {"speed_limit_kph": "km/h", "ego_speed_kph": "km/h"}
    measures = {"vehicle_ahead_gap_m": "m", "vehicle_ahead_relative_speed_kph": "km/h",
                "crossing_pedestrian_gap_m": "m", "crossing_pedestrian_speed_ms": "m/s"}
    return openodd.Backend(
        probes={n: ((lambda c, n=n: c.get(n)), units.get(n, "")) for n in names},
        readers={"measure": lambda k: ((lambda c, k=k: c.get(k)), measures[k])},
    )


def test_default_and_none_are_the_bundled_odd() -> None:
    backend = _bundled_backend()
    bundled = openodd.bundled_odd(backend=backend)
    for spec in (None, "", "default", " default "):
        assert openodd.odd_builder(spec) is None
        assert openodd.resolve_odd(spec, backend=backend).revision == bundled.revision


def test_an_odd_is_returned_as_is() -> None:
    odd = small_odd()
    assert openodd.resolve_odd(odd) is odd


def test_module_function_spec(monkeypatch: pytest.MonkeyPatch) -> None:
    module = types.ModuleType("my_odds")
    module.small_odd = small_odd  # type: ignore[attr-defined]
    monkeypatch.setitem(sys.modules, "my_odds", module)
    assert openodd.odd_builder("my_odds:small_odd") is small_odd
    assert openodd.resolve_odd("my_odds:small_odd").name == "small"
    with pytest.raises(ValueError, match="cannot import"):
        openodd.resolve_odd("my_odds:missing")


def test_a_builder_must_return_an_odd() -> None:
    openodd.register_odd("bad", lambda: 42)  # type: ignore[arg-type,return-value]
    with pytest.raises(ValueError, match="returned int"):
        openodd.resolve_odd("bad")


def test_yaml_paths(tmp_path: Path) -> None:
    doc = tmp_path / "odd.yaml"
    doc.write_text("TAXONOMY:\n  road: [urban, rural]\nODD:\n  urban:\n    INCLUDE_AND: {road: [urban]}\n")
    for spec in (str(doc), doc):
        assert openodd.odd_builder(spec) is None
        assert [m.name for m in openodd.resolve_odd(spec).modules] == ["urban"]
    with pytest.raises(ValueError, match=".yaml or .yml"):
        openodd.odd_builder(tmp_path / "odd.json")


def test_entry_points(monkeypatch: pytest.MonkeyPatch) -> None:
    loaded: list[str] = []

    class EntryPoint:
        def __init__(self, name: str, register) -> None:
            self.name, self.value, self._register = name, f"{name}:register", register

        def load(self):
            return self._register

    def broken() -> None:
        raise RuntimeError("broken plugin")

    points = {
        "openodd.odds": [EntryPoint("ok", lambda: openodd.register_odd("small", small_odd))],
        "my_app.odds": [
            EntryPoint("broken", broken),
            EntryPoint("app", lambda: openodd.register_odd("app", small_odd)),
        ],
    }

    def entry_points(*, group: str) -> list[EntryPoint]:
        loaded.append(group)
        return points.get(group, [])

    import importlib.metadata

    monkeypatch.setattr(importlib.metadata, "entry_points", entry_points)
    assert "small" in openodd.odd_names()
    assert "app" not in openodd.odd_names()
    openodd.add_entry_point_group("my_app.odds")
    openodd.add_entry_point_group("my_app.odds")
    assert {"small", "app"} <= set(openodd.registered_odds())
    assert loaded == ["openodd.odds", "my_app.odds"]
