"""Merge random coverage documents with one implementation; write the reports.

    python report_run.py ref|new OUT.json [CASES]
"""

from __future__ import annotations

import json
import random
import sys
from pathlib import Path
from typing import Any


def documents(seed: int) -> list[dict[str, Any]]:
    rng = random.Random(seed)
    items = []
    for i in range(rng.randint(1, 4)):
        buckets = [f"b{j}" for j in range(rng.randint(1, 5))]
        cover_by = rng.choice(["hits", "hits", "seconds", "entries"])
        items.append({
            "name": f"odd.item{i}", "group": rng.choice(["odd", "situation", "scenario"]),
            "kind": rng.choice(["numeric", "categorical"]), "event": rng.choice(["tick", "end"]),
            "buckets": buckets, "target": rng.choice([1, 2, 3]), "cover_by": cover_by,
            "min_stay": rng.choice([None, None, 1.0]), "unit": rng.choice(["", "m"]),
            "outside_odd": rng.sample(buckets, rng.randint(0, 1)), "text": "",
        })
    docs = []
    for run in range(rng.randint(1, 6)):
        doc_items = []
        for it in items:
            if rng.random() < 0.15:
                continue  # an item a run did not have
            raw = dict(it)
            if rng.random() < 0.1:
                raw = {**raw, "buckets": raw["buckets"] + ["extra"]}  # defined differently
            raw["hits"] = {b: rng.choice([0, 0, 1, 3, 10]) for b in raw["buckets"]}
            if rng.random() < 0.8:
                for m in ("seconds", "meters", "entries"):
                    raw[m] = {b: (rng.choice([0, 1, 2]) if m == "entries" else round(rng.uniform(0, 5), 2)) for b in raw["buckets"]}
            if raw["min_stay"] is not None:
                raw["counted"] = {raw["cover_by"]: {b: rng.choice([0, 1, 2]) for b in raw["buckets"]}}
            raw["samples"] = rng.randint(0, 20)
            if rng.random() < 0.2:
                raw["out_of_range"] = {"<above>": rng.randint(1, 3)}
            doc_items.append(raw)
        odd = None
        if rng.random() < 0.7:
            outside = rng.choice([0, 0, 3])
            odd = {"name": "urban", "text": "Urban", "ticks": {"inside": 10, "assumed": 2, "outside": outside},
                   "seconds": {"inside": 0.5, "assumed": 0.1, "outside": outside * 0.05},
                   "module_ticks": {"m": {"failed_ticks": outside, "missing_ticks": 1}},
                   "out_intervals": [[1.0, 1.5]] if outside else [], "unmeasured": ["x"] if run else []}
        docs.append({"schema": "autoware_carla_scenario.coverage/1", "scenario": f"S{run % 2}",
                     "items": doc_items, "crosses": [], "odd": odd})
    return docs


def main() -> None:
    impl, out, n = sys.argv[1], sys.argv[2], int(sys.argv[3]) if len(sys.argv) > 3 else 300
    if impl == "ref":
        from autoware_carla_scenario.coverage.report import merge_coverage
    else:
        from openodd import merge_coverage
    results = {}
    for seed in range(n):
        report = merge_coverage(documents(seed))
        results[seed] = {"markdown": report.to_markdown(max_holes=3),
                         "grades": {g: round(report.group_grade(g), 9) for g in report.groups()}}
    Path(out).write_text(json.dumps(results, indent=1))


if __name__ == "__main__":
    main()
