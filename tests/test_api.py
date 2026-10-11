"""The Python API over the Rust core."""

from __future__ import annotations

import enum
from pathlib import Path

import pytest

import openodd
from openodd import (
    UNDECIDED,
    Backend,
    CoverItem,
    OddAttribute,
    OddDefinition,
    OddModule,
    OpenOddError,
    SamplingEvent,
    all_of,
    any_of,
    module_holds,
)

TAXONOMY = """
TAXONOMY:
  speed: float velocity
  rain_rate: float precipitation_rate
  rain:
    dry: {rain_rate: "< 0.5 mm/h"}
    wet: {rain_rate: ">= 0.5 mm/h"}
  road: [urban, rural, highway]
  gap: float length
"""

MODULES = """
IMPORT: [taxonomy.yaml]
ODD:
  urban_odd:
    METADATA: {version: "2.1.0", owner: odd-team}
    INCLUDE_AND:
      slow: true
      road: [urban, rural]
    EXCLUDE_OR:
      rain: wet
MODULES:
  slow:
    INCLUDE_AND:
      speed: "< 50 km/h"
  close:
    INCLUDE_AND:
      gap: "< 10 m"
"""


def backend() -> Backend:
    return Backend(
        name="test",
        probes={
            "speed_kph": (lambda c: c.get("speed"), "km/h"),
            "rain_mm_h": (lambda c: c.get("rain_rate"), "mm/h"),
            "road": (lambda c: c.get("road"), ""),
        },
        readers={"measure": lambda key: ((lambda c, k=key: c.get(k)), "m")},
    )


@pytest.fixture
def binding(tmp_path: Path) -> Path:
    (tmp_path / "taxonomy.yaml").write_text(TAXONOMY)
    (tmp_path / "odd.yaml").write_text(MODULES)
    path = tmp_path / "urban.yaml"
    path.write_text(
        "openodd: [odd.yaml]\n"
        "text: Urban roads\n"
        "probes:\n"
        "  speed: {probe: speed_kph, buckets: [0, 30, 50, 80]}\n"
        "  rain_rate: {probe: rain_mm_h}\n"
        "  road: {probe: road}\n"
        "  gap: {measure: gap_m}\n"
        "situations:\n"
        "  close: {target: 2}\n"
    )
    return path


def test_a_binding_file_reads_an_odd(binding: Path) -> None:
    odd = openodd.load_odd_binding(binding, backend=backend())
    assert odd.name == "urban" and odd.text == "Urban roads"
    assert odd.roots == ["urban_odd"]
    assert [a.name for a in odd.attributes] == ["speed", "rain_rate", "road", "gap", "rain"]
    assert odd.attributes[0].unit == "km/h"
    assert [m.name for m in odd.situations()] == ["close"]
    assert odd.situations()[0].item.target == 2


def test_values_are_judged_with_missing_value_semantics(binding: Path) -> None:
    odd = openodd.load_odd_binding(binding, backend=backend())
    inside = odd.evaluate(odd.sample({"speed": 40, "road": "urban", "rain_rate": 0.0}))
    assert inside.inside and not inside.assumed
    assert not odd.evaluate(odd.sample({"speed": 60, "road": "urban"})).inside
    assert not odd.evaluate(odd.sample({"speed": 40, "road": "urban", "rain_rate": 1.0})).inside
    assumed = odd.evaluate(odd.sample({}))
    assert assumed.inside and assumed.assumed
    assert assumed.modules["slow"] is None


def test_a_derived_categorical_is_worked_out_from_its_number(binding: Path) -> None:
    odd = openodd.load_odd_binding(binding, backend=backend())
    assert odd.sample({"rain_rate": 0.7})["rain"] == "wet"
    assert odd.sample({})["rain"] is None


def test_buckets_outside_the_odd(binding: Path) -> None:
    odd = openodd.load_odd_binding(binding, backend=backend())
    assert odd.outside_buckets("speed") == ["[50, 80]"]
    assert odd.outside_buckets("road") == ["highway"]
    assert odd.outside_buckets("rain") == ["wet"]
    items = {i.name: i for i in odd.cover_items()}
    assert items["odd.speed"].bucket_of(49.9) == "[30, 50)"
    assert items["odd.speed"].bucket_of(90) == "<above>"


def test_the_revision_is_the_files(binding: Path, tmp_path: Path) -> None:
    odd = openodd.load_odd_binding(binding, backend=backend())
    again = openodd.load_odd_binding(binding, backend=backend())
    assert odd.revision == again.revision and len(odd.revision) == 64
    assert odd.version == "2.1.0"
    assert [f["name"] for f in odd.files] == ["urban.yaml", "odd.yaml", "taxonomy.yaml"]
    (tmp_path / "taxonomy.yaml").write_text(TAXONOMY + "  extra: boolean\n")
    changed = openodd.load_odd_binding(binding, backend=backend())
    assert changed.revision != odd.revision


def test_an_archive_reads_back_to_the_same_odd(binding: Path) -> None:
    odd = openodd.load_odd_binding(binding, backend=backend())
    archive = odd.archive()
    assert archive == odd.archive()  # deterministic
    manifest = openodd.read_archive_manifest(archive)
    assert manifest["revision"] == odd.revision and manifest["version"] == "2.1.0"
    back = openodd.load_odd_archive(archive, backend=backend())
    assert back.revision == odd.revision
    assert back.structure_digest == odd.structure_digest
    assert back.evaluate(back.sample({"speed": 60})).inside is False


def test_an_unknown_probe_is_refused(tmp_path: Path) -> None:
    (tmp_path / "odd.yaml").write_text(TAXONOMY)
    (tmp_path / "b.yaml").write_text("openodd: [odd.yaml]\nprobes:\n  speed: {probe: nope}\n")
    with pytest.raises(OpenOddError, match="unknown probe 'nope'"):
        openodd.load_odd_binding(tmp_path / "b.yaml", backend=backend())


def test_a_key_written_twice_is_refused() -> None:
    with pytest.raises(OpenOddError, match="written twice"):
        openodd.load_openodd("TAXONOMY: {flag: boolean}\nMODULES:\n  m: {INCLUDE_AND: {flag: true, flag: false}}\n")


class Rain(enum.Enum):
    NONE = 0
    HEAVY = 1


def test_an_odd_built_in_code() -> None:
    speed = OddAttribute("speed", lambda c: c["speed"], unit="km/h", buckets=[0, 30, 60])
    rain = OddAttribute("rain", lambda c: c["rain"], values=list(Rain))
    slow = OddModule("slow", include_and=[speed.less_than(30)])
    dry = OddModule("dry", exclude_or=[rain.is_in([Rain.HEAVY])], labels=["fair"])
    root = OddModule("root", include_and=[module_holds("slow"), module_holds("fair")])
    odd = OddDefinition("code", [speed, rain], [slow, dry, root])
    assert odd.roots == ["root"]
    assert odd.revision is None and len(odd.structure_digest) == 64
    assert odd.evaluate({"speed": 20, "rain": Rain.NONE}).inside
    assert not odd.evaluate({"speed": 20, "rain": Rain.HEAVY}).inside
    assert odd.outside_buckets(rain) == ["HEAVY"]
    assert odd.outside_buckets(speed) == ["[30, 60]"]
    assert rain.item.labels == ["NONE", "HEAVY"]


def test_groups_and_inactive_modules() -> None:
    x = OddAttribute("x", lambda c: c)
    off = OddModule("off", include_and=[x.at_least(10)], active=False)
    root = OddModule("root", include_or=[module_holds("off"), all_of([x.at_most(5), any_of([x.equals(1), x.equals(2)])])])
    odd = OddDefinition("g", [x], [off, root])
    assert odd.evaluate({"x": 2}).inside
    assert not odd.evaluate({"x": 3}).inside
    assert odd.evaluate({"x": UNDECIDED}).assumed


def test_cycles_are_refused() -> None:
    a = OddModule("a", include_and=[module_holds("b")])
    b = OddModule("b", include_and=[module_holds("a")])
    with pytest.raises(ValueError, match="refer to each other"):
        OddDefinition("c", [], [a, b])


def test_cover_items_of_a_backend() -> None:
    class Fired:
        label = "cut_in"

    item = CoverItem("gap", lambda c: c, range=(0, 25), every=10, event=Fired())
    assert item.labels == ["[0, 10)", "[10, 20)", "[20, 25]"]
    assert item.describe()["event"] == "condition:cut_in"
    with pytest.raises(ValueError, match="SamplingEvent.TICK"):
        CoverItem("t", lambda c: c, values=[1], cover_by="seconds", event=Fired())
    assert CoverItem("t", lambda c: c, values=[1], cover_by="seconds", event=SamplingEvent.TICK)


def test_units() -> None:
    assert openodd.convert(36, "km/h", "m/s") == pytest.approx(10)
    assert openodd.normalize_unit("kph") == "km/h"
    with pytest.raises(openodd.UnitError):
        openodd.convert(1, "m", "s")
