"""Write tests/rules_golden.json: simpful's outputs for rule files, to hold the
Rust rules (src/rules.rs) to.

Run with a Python that imports simpful and pyyaml (neither is a dependency)::

    uv run --no-project --with simpful --with pyyaml python tools/rules_golden.py

Each rule base of each file is built as a simpful Sugeno system and inferred
at seeded random points, inputs clamped to their universes first (which the
Rust rules do and simpful does not). The rules are written as simpful parses
them: it refuses a ``NOT`` clause as the first operand of ``AND`` or ``OR``,
which the Rust rules read.
"""

from __future__ import annotations

import contextlib
import io
import json
import random
from pathlib import Path

import yaml
from simpful import (
    FuzzySet,
    FuzzySystem,
    Gaussian_MF,
    LinguisticVariable,
    Trapezoidal_MF,
    Triangular_MF,
)

OUT = Path(__file__).resolve().parent.parent / "tests" / "rules_golden.json"
POINTS = 200

FILES = {
    "grip": """
version: 1
quantities:
  grip:
    rule_bases:
      water:
        inputs:
          water:
            universe: [0, 100]
            terms:
              dry: {trapezoid: [0, 0, 0, 50]}
              wet: {triangle: [0, 50, 100]}
              flooded: {trapezoid: [50, 100, 100, 100]}
        rules:
          - if: (water IS dry)
            then: {low: 0.90, high: 1.10}
          - if: (water IS wet)
            then: {low: 0.65, high: 0.85}
          - if: (water IS flooded)
            then: {low: 0.45, high: 0.65}
""",
    "operators": """
version: 1
quantities:
  q:
    rule_bases:
      b:
        inputs:
          x:
            universe: [-10, 10]
            terms:
              neg: {gaussian: [-10, 6]}
              zero: {gaussian: [0, 3]}
              pos: {trapezoid: [0, 6, 10, 10]}
          y:
            universe: [0, 1]
            terms:
              lo: {triangle: [0, 0, 0.7]}
              hi: {triangle: [0.3, 1, 1]}
        rules:
          - if: (x IS neg) AND (y IS lo)
            then: {a: -1, b: 3}
          - if: (x IS zero) OR (y IS hi)
            then: {a: 0.5, b: 2}
          - if: ((y IS lo) OR (x IS pos)) AND (NOT (x IS neg))
            then: {a: 2, b: -4}
          - if: NOT ((x IS zero) AND (y IS hi))
            then: {a: 0, b: 1}
""",
}


def _mf(spec: dict):  # type: ignore[no-untyped-def]
    ((shape, p),) = spec.items()
    if shape == "triangle":
        return Triangular_MF(*p)
    if shape == "trapezoid":
        return Trapezoidal_MF(*p)
    return Gaussian_MF(mu=p[0], sigma=p[1])


def main() -> None:
    rng = random.Random(0)
    cases = []
    for name, text in FILES.items():
        doc = yaml.safe_load(text)
        for quantity, q in doc["quantities"].items():
            for base_name, base in q["rule_bases"].items():
                with contextlib.redirect_stdout(io.StringIO()):
                    fs = FuzzySystem(show_banner=False, verbose=False)
                    for var, i in base["inputs"].items():
                        fs.add_linguistic_variable(
                            var,
                            LinguisticVariable(
                                [FuzzySet(function=_mf(s), term=t) for t, s in i["terms"].items()],
                                universe_of_discourse=i["universe"],
                            ),
                        )
                    outputs = list(base["rules"][0]["then"])
                    texts = []
                    for k, rule in enumerate(base["rules"]):
                        for out, value in rule["then"].items():
                            fs.set_crisp_output_value(f"r{k}_{out}", value)
                            texts.append(f"IF ({rule['if']}) THEN ({out} IS r{k}_{out})")
                    fs.add_rules(texts)
                points = []
                for _ in range(POINTS):
                    given, clamped = {}, {}
                    for var, i in base["inputs"].items():
                        low, high = i["universe"]
                        span = high - low
                        v = rng.uniform(low - 0.1 * span, high + 0.1 * span)
                        given[var] = v
                        clamped[var] = min(max(v, low), high)
                    for var, v in clamped.items():
                        fs.set_variable(var, v)
                    out = fs.Sugeno_inference(outputs)
                    points.append(
                        {"inputs": given, "outputs": {o: float(out[o]) for o in outputs}}
                    )
                cases.append({"file": name, "quantity": quantity, "points": points})
    OUT.write_text(json.dumps({"files": FILES, "cases": cases}, indent=1) + "\n")
    print(f"wrote {OUT}: {sum(len(c['points']) for c in cases)} points")


if __name__ == "__main__":
    main()
