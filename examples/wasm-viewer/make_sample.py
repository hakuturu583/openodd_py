"""Write sample.json: the coverage files of a synthetic cut-in sweep, for the
WASM viewer demo. Deterministic; needs only the openodd package.

    python examples/wasm-viewer/make_sample.py

Each run records its ODD cells (gap, relative speed, rain), its case (the
values it was drawn with, why, and the space) and its outcome. A run passes
when the gap is long enough for the closing speed -- less so in heavy rain --
with some noise near that boundary; a few cases are run again. The scenario
stages every gap 1.5 m longer than drawn, for the drift view to find.
"""

from __future__ import annotations

import json
import math
import random
from pathlib import Path
from types import SimpleNamespace

import openodd
from openodd import OddAttribute, OddDefinition, OddModule

HERE = Path(__file__).resolve().parent
#: How much longer the scenario stages the gap than drawn: what the
#: drift view finds.
STAGING_OFFSET = 1.5


def odd() -> OddDefinition:
    gap = OddAttribute("gap", lambda c: c["gap"], unit="m", buckets=[0, 10, 20, 30, 50])
    speed = OddAttribute("relative_speed", lambda c: c["speed"], unit="km/h", buckets=[-30, -15, -5, 5, 15])
    rain = OddAttribute("rain", lambda c: c["rain"], values=["none", "light", "heavy"])
    return OddDefinition("cut_in", [gap, speed, rain], [OddModule("root")])


def bucket(value: float, edges: list[float]) -> str:
    for lo, hi in zip(edges, edges[1:]):
        if lo <= value < hi:
            return f"[{lo:g}, {hi:g})"
    return f"[{edges[-2]:g}, {edges[-1]:g}]"


def main() -> None:
    rng = random.Random(7)
    space = openodd.ParameterSpace(numeric={"gap": (6.0, 30.0), "relative_speed": (-15.0, 5.0)},
                                   categorical={"rain": ["none", "light", "heavy"]})
    cases = []
    for i in range(30):
        cases.append({"gap": round(rng.uniform(6, 30), 2), "relative_speed": round(rng.uniform(-15, 5), 2),
                      "rain": rng.choice(["none", "light", "heavy"]), "chosen_for": "coverage" if i < 18 else "instability"})
    runs = [(c, 1) for c in cases] + [(cases[i], 2) for i in (2, 5, 9, 21, 24)]
    docs = []
    for n, (case, _) in enumerate(runs):
        need = 12 - 0.6 * case["relative_speed"] + {"none": 0, "light": 2, "heavy": 6}[case["rain"]]
        p_pass = 1 / (1 + math.exp(-(case["gap"] - need) / 1.5))
        passed = rng.random() < p_pass
        collector = openodd.CoverageCollector([], odd=odd())
        flaky = None
        if case["chosen_for"] == "instability":
            flaky = openodd.FlakyMetrics(pass_probability=round(p_pass, 3), flip_probability=0.2,
                                    instability_confidence=0.6, robustness_radius=0.05)
        collector.record_case(SimpleNamespace(
            index=n, situation=None,
            values={k: case[k] for k in ("gap", "relative_speed", "rain")},
            buckets={"gap": bucket(case["gap"], [0, 10, 20, 30, 50]),
                     "relative_speed": bucket(case["relative_speed"], [-30, -15, -5, 5, 15]),
                     "rain": case["rain"]},
            overrides=[f"scenario.gap_m={case['gap']}", f"scenario.relative_speed_kmh={case['relative_speed']}",
                       f"environment.rain={case['rain']}"],
            flaky=flaky), space=space)
        # The run closes the gap from what the scenario staged: 1.5 m more
        # than drawn (a knob whose offset is off), its cells moving.
        t, gap = 0.0, case["gap"] + STAGING_OFFSET
        collector.start({"gap": gap, "speed": case["relative_speed"], "rain": case["rain"]}, 0.0)
        for _ in range(40):
            t += 0.1
            gap = max(0.5, gap + case["relative_speed"] / 3.6 * 0.1)
            collector.tick({"gap": gap, "speed": case["relative_speed"], "rain": case["rain"]}, t)
        collector.end({}, t)
        collector.record_outcome(passed, failed_at=None if passed else round(t * 0.8, 2), environment="sample")
        docs.append(collector.to_dict("CutIn"))
    out = HERE / "sample.json"
    out.write_text(json.dumps(docs, separators=(",", ":")), encoding="utf-8")
    print(f"{out}: {len(docs)} runs, {out.stat().st_size // 1024} KiB")


if __name__ == "__main__":
    main()
