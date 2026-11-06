"""Units from an EA taxonomy model (#45)."""

from __future__ import annotations

from pathlib import Path

import pytest

import openodd
from openodd.taxonomy_units import conversion_block, conversion_yaml, main, read_units

MODEL = Path(__file__).parent / "data" / "units_model.xml"


def test_each_unit_is_read_once() -> None:
    units = read_units(MODEL)
    assert [u.symbol for u in units] == ["mm/h", "in/h", "m", "nt", "knt"]
    assert units[2].unit_types == ("length", "distance")


def test_units_convert_from_the_first_of_their_type() -> None:
    block = conversion_block(read_units(MODEL))
    assert block["precipitation_rate"]["mm/h"]["in/h"]["scale"] == pytest.approx(1 / 25.4)
    assert block["luminance"] == {"nt": {"knt": {"scale": pytest.approx(0.001), "offset": 0.0}}}
    # A type with one unit names it on its own.
    assert block["length"] == {"m": {"m": {"scale": 1.0, "offset": 0.0}}}


def test_an_odd_reads_the_units(tmp_path: Path) -> None:
    assert main([str(MODEL), "-o", str(tmp_path / "units.yaml")]) == 0
    (tmp_path / "taxonomy.yaml").write_text(
        """
IMPORT: [units.yaml]
TAXONOMY:
  rate: float precipitation_rate
  glare: float luminance
MODULES:
  dry:
    INCLUDE_AND:
      rate: "< 0.1 in/h"
      glare: "< 2 knt"
"""
    )
    odd = openodd.load_openodd(
        tmp_path / "taxonomy.yaml",
        bindings={"rate": {"probe": "rain"}, "glare": {"probe": "glare"}},
        backend=openodd.Backend(
            name="test",
            probes={
                "rain": (lambda c: c.get("rain"), "mm/h"),
                "glare": (lambda c: c.get("glare"), "nt"),
            },
        ),
    )
    # 0.1 in/h is 2.54 mm/h; 2 knt is 2000 nt.
    assert odd.evaluate(odd.sample({"rain": 2.5, "glare": 1999})).inside
    assert not odd.evaluate(odd.sample({"rain": 2.6, "glare": 1999})).inside
    assert not odd.evaluate(odd.sample({"rain": 2.5, "glare": 2001})).inside


def test_the_text_names_its_source() -> None:
    text = conversion_yaml(read_units(MODEL), source="units_model.xml")
    assert text.startswith("# Units of units_model.xml")
    assert '"mm/h":' in text


def test_a_file_without_units_is_refused(tmp_path: Path, capsys: pytest.CaptureFixture[str]) -> None:
    (tmp_path / "empty.xml").write_text("<TAXONOMY/>")
    assert main([str(tmp_path / "empty.xml")]) == 1
    assert "no units found" in capsys.readouterr().err
