"""This library against the reference implementation, case by case.

The reference is autoware_carla_scenario's ``odd`` package, which this library
was ported from. Set ``OPENODD_REFERENCE_PYTHON`` to a Python that imports it
(and ``OPENODD_REFERENCE_PATH`` to put its source on the path) to run.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path

import pytest

HERE = Path(__file__).parent / "differential"
REFERENCE = os.environ.get("OPENODD_REFERENCE_PYTHON")
RANDOM_CASES = int(os.environ.get("OPENODD_DIFFERENTIAL_CASES", "300"))


@pytest.mark.skipif(not REFERENCE, reason="OPENODD_REFERENCE_PYTHON is not set")
def test_the_reference_and_this_library_agree(tmp_path: Path) -> None:
    env = {**os.environ, "PYTHONPATH": os.pathsep.join(
        p for p in (os.environ.get("OPENODD_REFERENCE_PATH"), str(HERE)) if p)}
    for impl, python in (("ref", REFERENCE), ("new", sys.executable)):
        subprocess.run([python, str(HERE / "run.py"), impl, str(tmp_path / f"{impl}.json"), str(RANDOM_CASES)],
                       check=True, env=env)
    done = subprocess.run([sys.executable, str(HERE / "compare.py"), str(tmp_path / "ref.json"), str(tmp_path / "new.json")],
                          capture_output=True, text=True)
    assert done.returncode == 0, done.stdout[-4000:]
    ref = json.loads((tmp_path / "ref.json").read_text())
    assert len(ref) == RANDOM_CASES + 11


@pytest.mark.skipif(not REFERENCE, reason="OPENODD_REFERENCE_PYTHON is not set")
def test_coverage_merges_agree(tmp_path: Path) -> None:
    env = {**os.environ, "PYTHONPATH": os.pathsep.join(
        p for p in (os.environ.get("OPENODD_REFERENCE_PATH"), str(HERE)) if p)}
    for impl, python in (("ref", REFERENCE), ("new", sys.executable)):
        subprocess.run([python, str(HERE / "report_run.py"), impl, str(tmp_path / f"{impl}.json"), "300"],
                       check=True, env=env, capture_output=True)
    ref = json.loads((tmp_path / "ref.json").read_text())
    new = json.loads((tmp_path / "new.json").read_text())
    differ = [k for k in ref if ref[k] != new[k]]
    assert not differ, f"{len(differ)} merges differ, first: {differ[:3]}"
