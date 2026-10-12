"""Run the differential cases on one implementation; write what it made of them.

    python run.py ref|new OUT.json [RANDOM_CASES]

``ref`` is the reference implementation (``autoware_carla_scenario.odd``),
``new`` this library. The output is plain JSON, so the two can be compared.
"""

from __future__ import annotations

import json
import math
import sys
import tempfile
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).parent))
from cases import all_cases, contexts  # noqa: E402

#: Keys of an item's definition the reference does not write.
NEWER_KEYS = ("amount", "attribute")


def plain(x: Any) -> Any:
    if isinstance(x, float):
        return None if math.isnan(x) else round(x, 9)
    if isinstance(x, dict):
        return {str(k): plain(v) for k, v in x.items()}
    if isinstance(x, (list, tuple)):
        return [plain(v) for v in x]
    if x is None or isinstance(x, (bool, int, str)):
        return x
    return repr(x)


def run(impl: str, case: dict[str, Any]) -> dict[str, Any]:
    if impl == "ref":
        from autoware_carla_scenario.odd import load_odd_binding
    else:
        from openodd import load_odd_binding
    with tempfile.TemporaryDirectory() as tmp:
        for name, text in case["files"].items():
            (Path(tmp) / name).write_text(text)
        try:
            odd = load_odd_binding(Path(tmp) / case["entry"])
        except Exception as exc:  # the error, not its wording, must agree
            return {"error": type(exc).__name__, "message": str(exc).replace(tmp, "<tmp>")}
    out: dict[str, Any] = {
        "name": odd.name,
        "text": odd.text,
        "roots": list(odd.roots),
        "attributes": [
            {
                "name": a.name,
                "unit": a.unit,
                "labels": a.item.labels if a.item is not None else None,
                "outside": odd.outside_buckets(a),
                "target": a.item.target if a.item is not None else None,
                "cover_by": a.item.cover_by if a.item is not None else None,
                "min_stay": a.item.min_stay if a.item is not None else None,
            }
            for a in odd.attributes
        ],
        "modules": [m.describe() for m in odd.modules],
        # amount / attribute came after the reference: compare what both write.
        "situations": [{k: v for k, v in m.item.describe().items() if k not in NEWER_KEYS}
                       for m in odd.situations()],
        "unmeasured": odd.unmeasured(),
    }
    verdicts = []
    for ctx in contexts(case):
        values = odd.sample(ctx)
        v = odd.evaluate(values)
        verdicts.append({"values": values, "inside": v.inside, "assumed": v.assumed, "modules": v.modules})
    out["verdicts"] = verdicts
    return plain(out)


def main() -> None:
    impl, path = sys.argv[1], sys.argv[2]
    randoms = int(sys.argv[3]) if len(sys.argv) > 3 else 300
    results = {case["id"]: run(impl, case) for case in all_cases(randoms)}
    Path(path).write_text(json.dumps(results, indent=1, sort_keys=True))


if __name__ == "__main__":
    main()
