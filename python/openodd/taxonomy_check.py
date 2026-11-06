"""Which concepts an ODD, its bindings and its rules use that a reference
taxonomy lacks, or means otherwise.

A shared taxonomy -- tier4/taxonomy's, say -- is the vocabulary ODDs and
rules are written in. A concept used here and missing there, or named alike
and meaning something else, is where the two drift apart: it should be added
to the taxonomy, through an issue on it, or renamed here::

    openodd-taxonomy-check taxonomy_openodd.yaml --odd my_odd.binding.yaml \\
        --rules weather_rules.yaml --drafts drafts/

It reports, for each finding:

* ``missing``: a concept the files use that the reference does not declare;
* ``differs``: a concept the reference declares otherwise -- of another kind,
  another unit dimension, or other literals;
* ``unclassified``: a rule input that stands for no concept (version 2
  rules: docs/rules.md, "Inputs and the taxonomy").

With ``--drafts``, it writes for each missing concept the text of an issue
proposing it to the reference. The command exits 1 when it finds anything,
so CI can run it.
"""

from __future__ import annotations

import argparse
import os
import sys
from collections.abc import Iterable, Mapping, Sequence
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Optional, Union

from . import _core
from .backend import Backend
from .loader import load_odd_file, load_openodd
from .model import OddDefinition
from .rules import Rules, RulesError

__all__ = ["Finding", "TaxonomyCheck", "check_taxonomy", "main"]

PathLike = Union[str, "os.PathLike[str]"]


class _AnyProbe(Backend):
    """Reads a binding without measuring anything: every probe and reader is
    taken as it is named."""

    def reader_keys(self) -> list[str]:
        return ["measure"]

    def probe(self, spec: str) -> Any:
        return (lambda context: None), ""

    def read(self, key: str, value: str) -> Any:
        return (lambda context: None), ""


@dataclass(frozen=True)
class Finding:
    """One concept where the files and the reference part."""

    #: ``missing``, ``differs`` or ``unclassified``.
    kind: str
    concept: str
    #: Where it is used: the file, and what uses it there.
    where: str
    #: What differs, or what the files say the concept is.
    detail: str
    #: What the files declare it to be (:attr:`.OddAttribute.concept`).
    declared: Mapping[str, Any] = field(default_factory=dict)

    def __str__(self) -> str:
        return f"{self.kind}: {self.concept} ({self.where}): {self.detail}"


@dataclass
class TaxonomyCheck:
    """The findings of :func:`check_taxonomy`, and the reference checked against."""

    reference: str
    findings: list[Finding]

    @property
    def ok(self) -> bool:
        return not self.findings

    def missing(self) -> list[Finding]:
        return [f for f in self.findings if f.kind == "missing"]

    def draft(self, finding: Finding) -> str:
        """The text of an issue proposing *finding*'s concept to the reference."""
        d = finding.declared
        lines = [
            f"# Add `{finding.concept}` to the taxonomy",
            "",
            "## Concept",
            "",
            f"- Name: `{finding.concept}`",
            f"- Kind: {d.get('kind', 'unknown')}",
        ]
        if d.get("kind") == "number":
            lines.append(f"- Type: `{d.get('primitive', 'float')} {d.get('unit_type', '')}`".rstrip())
        if d.get("literals"):
            lines.append("- Literals: " + ", ".join(f"`{x}`" for x in d["literals"]))
        lines += [
            "",
            "## Why it is needed",
            "",
            f"It is used by {finding.where}, and {self.reference} does not declare it: an ODD or a "
            "rule names a condition the shared vocabulary cannot. (Fill in what it describes, and "
            "why the ODD needs it.)",
            "",
        ]
        return "\n".join(lines)


def _declared(odd: OddDefinition) -> dict[str, dict[str, Any]]:
    """Concept -> what the taxonomy of *odd* declares it to be."""
    return {a.name: a.concept for a in odd.attributes}


def _compare(name: str, ours: Mapping[str, Any], theirs: Mapping[str, Any]) -> Optional[str]:
    """What differs between two declarations of *name*, or ``None``."""
    kinds = {ours.get("kind"), theirs.get("kind")}
    # A categorical defined by ranges is still a categorical.
    if kinds <= {"categorical", "derived"}:
        a, b = list(ours.get("literals") or []), list(theirs.get("literals") or [])
        if a and b and set(a) != set(b):
            return f"literals {a} here, {b} in the reference"
        return None
    if len(kinds) > 1:
        return f"a {ours.get('kind')} here, a {theirs.get('kind')} in the reference"
    if ours.get("kind") == "number":
        mine, its = (str(x.get("unit_type") or "") for x in (ours, theirs))
        if mine and its and _core.unit_dimension(mine) != _core.unit_dimension(its):
            return f"unit type {mine!r} here, {its!r} in the reference"
    return None


def check_taxonomy(
    reference: Union[PathLike, OddDefinition],
    *,
    odds: Iterable[PathLike] = (),
    rules: Iterable[PathLike] = (),
) -> TaxonomyCheck:
    """Check *odds* (OpenODD files or binding files) and *rules* (rule files)
    against the *reference* taxonomy (a file, or an ODD read from one).

    Raises:
        OpenOddError, RulesError: When a file cannot be read.
    """
    if isinstance(reference, OddDefinition):
        ref, label = reference, reference.name
    else:
        ref, label = load_openodd(Path(reference)), str(reference)
    theirs = _declared(ref)
    findings: list[Finding] = []
    for path in odds:
        odd = load_odd_file(Path(path), backend=_AnyProbe(name="check"))
        for name, ours in _declared(odd).items():
            if ours.get("kind") == "other":
                continue
            where = f"{path}"
            if name not in theirs:
                findings.append(Finding("missing", name, where, f"declared here as {ours.get('kind')}", ours))
                continue
            differs = _compare(name, ours, theirs[name])
            if differs:
                findings.append(Finding("differs", name, where, differs, ours))
    for path in rules:
        rule_set = Rules.read(path)
        for name in rule_set.unclassified_inputs:
            findings.append(Finding(
                "unclassified", name, f"{path} (input)",
                "a rule input that stands for no taxonomy concept (version 2 rules)",
            ))
        # What the rules' own taxonomy declares each concept to be: the
        # declaration the rules were checked against when read, kept by the
        # core (its label may be a git source, not a readable path).
        own = rule_set.taxonomy_concepts
        for name, (concept, _unit) in rule_set.input_concepts.items():
            ours = own.get(concept, {"kind": "number"})
            where = f"{path} (input {name})"
            if concept not in theirs:
                findings.append(Finding(
                    "missing", concept, where,
                    "a rule input's concept the reference does not declare", ours,
                ))
                continue
            differs = _compare(concept, ours, theirs[concept])
            if differs:
                findings.append(Finding("differs", concept, where, differs, ours))
    return TaxonomyCheck(reference=label, findings=findings)


def main(argv: Optional[Sequence[str]] = None, *, prog: str = "openodd-taxonomy-check") -> int:
    """``openodd-taxonomy-check``: what the files use that the reference lacks."""
    parser = argparse.ArgumentParser(
        prog=prog,
        description="Report the concepts ODDs, bindings and rule files use that a reference "
        "taxonomy lacks or declares otherwise; exit 1 when there are any.",
    )
    parser.add_argument("reference", type=Path, help="the reference taxonomy (an OpenODD file)")
    parser.add_argument("--odd", action="append", default=[], type=Path, metavar="FILE",
                        help="an OpenODD file or a binding file (repeatable)")
    parser.add_argument("--rules", action="append", default=[], type=Path, metavar="FILE",
                        help="a rule file (repeatable)")
    parser.add_argument("--drafts", type=Path, metavar="DIR",
                        help="write an issue draft per missing concept into DIR")
    args = parser.parse_args(argv)
    try:
        result = check_taxonomy(args.reference, odds=args.odd, rules=args.rules)
    except (ValueError, RulesError) as e:
        print(f"{prog}: {e}", file=sys.stderr)
        return 2
    for f in result.findings:
        print(f)
    if args.drafts is not None:
        args.drafts.mkdir(parents=True, exist_ok=True)
        seen: set[str] = set()
        for f in result.missing():
            if f.concept in seen:
                continue
            seen.add(f.concept)
            (args.drafts / f"{f.concept}.md").write_text(result.draft(f), encoding="utf-8")
    print(f"{len(result.findings)} finding(s) against {result.reference}")
    return 0 if result.ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
