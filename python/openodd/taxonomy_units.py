"""Units from an EA taxonomy model, as an OpenODD ``conversion:`` block.

tier4/taxonomy's master model (``taxonomy_master.xml``) defines every unit it
writes: its symbol, its unit types, and how it relates to the type's SI unit
(``value = si * factor + offset``). This reads those definitions and writes
the ``conversion:`` block an OpenODD file carries, so the units an ODD reads
follow the model rather than this library's own table::

    openodd-taxonomy-units taxonomy_master.xml -o units.yaml

and then ``IMPORT: [units.yaml]`` in the taxonomy, or pass both files to
:func:`openodd.load_openodd`.

Within a unit type, the first unit the model names anchors the others: each
other unit converts from it. A type with one unit names it on its own, which
makes it the type's unit. The SI unit itself goes unnamed in the model, so it
is not written.
"""

from __future__ import annotations

import argparse
import json
import sys
import xml.etree.ElementTree as ET
from dataclasses import dataclass, field
from pathlib import Path
from typing import Optional, Sequence, Union


@dataclass(frozen=True)
class ModelUnit:
    """One unit of the model: ``value = si * factor + offset``."""

    id: str
    symbol: str
    factor: float
    offset: float
    unit_types: tuple[str, ...] = field(default=())


def _text(element: ET.Element, tag: str) -> Optional[str]:
    child = element.find(tag)
    return None if child is None or child.text is None else child.text.strip()


def read_units(path: Union[str, Path]) -> list[ModelUnit]:
    """Every unit the model defines, once each, in the order first met."""
    found: dict[str, ModelUnit] = {}
    for element in ET.parse(path).iter():
        if not element.get("class", "").endswith(".TaxonomyUnit"):
            continue
        uid, symbol = _text(element, "id"), _text(element, "name")
        if not uid or not symbol or uid in found:
            continue
        types = element.find("reference_unit_types")
        unit_types = tuple(
            t for t in (_text(item, "id") for item in ([] if types is None else list(types))) if t
        )
        found[uid] = ModelUnit(
            id=uid,
            symbol=symbol,
            factor=float(_text(element, "factor") or 1.0),
            offset=float(_text(element, "offset") or 0.0),
            unit_types=unit_types,
        )
    return list(found.values())


def conversion_block(units: Sequence[ModelUnit]) -> dict[str, dict[str, dict[str, dict[str, float]]]]:
    """The ``conversion:`` block: unit type -> from -> to -> ``scale``, ``offset``."""
    by_type: dict[str, list[ModelUnit]] = {}
    for u in units:
        for t in u.unit_types:
            if all(other.symbol != u.symbol for other in by_type.setdefault(t, [])):
                by_type[t].append(u)
    block: dict[str, dict[str, dict[str, dict[str, float]]]] = {}
    for unit_type, members in by_type.items():
        anchor, others = members[0], members[1:]
        targets: dict[str, dict[str, float]] = {}
        if not others:
            targets[anchor.symbol] = {"scale": 1.0, "offset": 0.0}
        for u in others:
            # si = (anchor - oa) / fa, and u = si * fu + ou.
            scale = u.factor / anchor.factor
            targets[u.symbol] = {"scale": scale, "offset": u.offset - anchor.offset * scale}
        block[unit_type] = {anchor.symbol: targets}
    return block


def conversion_yaml(units: Sequence[ModelUnit], source: str = "") -> str:
    """The block as the text of an OpenODD file."""
    lines = []
    if source:
        lines.append(f"# Units of {source}, written by openodd-taxonomy-units.")
    lines.append("conversion:")
    for unit_type, sources in conversion_block(units).items():
        lines.append(f"  {json.dumps(unit_type)}:")
        for src, targets in sources.items():
            lines.append(f"    {json.dumps(src)}:")
            for dst, spec in targets.items():
                lines.append(
                    f"      {json.dumps(dst)}: {{scale: {spec['scale']!r}, offset: {spec['offset']!r}}}"
                )
    return "\n".join(lines) + "\n"


def main(argv: Optional[Sequence[str]] = None) -> int:
    parser = argparse.ArgumentParser(
        prog="openodd-taxonomy-units",
        description="Write the units an EA taxonomy model defines as an OpenODD conversion: block.",
    )
    parser.add_argument("model", type=Path, help="the model's XML export (taxonomy_master.xml)")
    parser.add_argument("-o", "--output", type=Path, help="where to write it (default: stdout)")
    args = parser.parse_args(argv)
    units = read_units(args.model)
    if not units:
        print(f"{args.model}: no units found", file=sys.stderr)
        return 1
    text = conversion_yaml(units, source=args.model.name)
    if args.output:
        args.output.write_text(text, encoding="utf-8")
    else:
        sys.stdout.write(text)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
