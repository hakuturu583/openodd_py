"""Compare two runs of ``run.py``: print every case where they disagree."""

import json
import sys
from typing import Any


def diff(a: Any, b: Any, path: str = "") -> list[str]:
    if isinstance(a, dict) and isinstance(b, dict):
        out = []
        for k in sorted(set(a) | set(b)):
            if k == "message":
                continue
            out += diff(a.get(k, "<absent>"), b.get(k, "<absent>"), f"{path}.{k}")
        return out
    if isinstance(a, list) and isinstance(b, list) and len(a) == len(b):
        return [d for i, (x, y) in enumerate(zip(a, b)) for d in diff(x, y, f"{path}[{i}]")]
    return [] if a == b else [f"{path}: ref={a!r} new={b!r}"]


ref, new = (json.load(open(p)) for p in sys.argv[1:3])
bad = 0
for case in ref:
    d = diff(ref[case], new.get(case))
    if d:
        bad += 1
        print(f"== {case}: {len(d)} differences")
        if "error" in ref[case] or "error" in (new.get(case) or {}):
            print("   ref:", ref[case].get("message", "ok"))
            print("   new:", (new.get(case) or {}).get("message", "ok"))
        for line in d[:8]:
            print("  ", line)
errors = sum("error" in r for r in ref.values())
print(f"\n{len(ref) - bad}/{len(ref)} cases agree ({errors} are errors in the reference)")
sys.exit(1 if bad else 0)
