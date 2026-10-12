"""Merge coverage files and grade them (the merge is the Rust core's).

A coverage file (``*_coverage.json``, schema :data:`COVERAGE_SCHEMA`) is what
the collector (:class:`~.collector.CoverageCollector`) writes for one run: every cover item's hits and
exposure per bucket, and the run's exposure to its ODD. Merged:

* a bucket is covered when its amount -- hits, or the seconds, metres or
  entries ``cover_by`` names, counting only stays of ``min_stay`` when given --
  reaches the item's ``target``;
* an item's grade is its covered buckets over those inside the ODD; a group's
  grade is the mean of its items';
* an item defined differently in different runs is reported once per
  definition (``name#2``), never merged.

A run that recorded the case it ran (:meth:`~.collector.CoverageCollector.record_case`)
is kept as a :class:`CaseRun`. Its flakiness is refitted over all the runs of
its scenario that were chosen in the same space and recorded an outcome
(``assessed``), and the report lists them, flakiest first.
"""

from __future__ import annotations

import argparse
import json
import logging
import sys
from collections.abc import Iterable, Sequence
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Optional

from . import _core

__all__ = [
    "COVERAGE_FILE_GLOB",
    "COVERAGE_SCHEMA",
    "COVERAGE_SCHEMAS",
    "CaseRun",
    "CoverageReport",
    "MergedEntry",
    "OddExposure",
    "find_coverage_files",
    "load_and_merge",
    "load_coverage_files",
    "main",
    "merge_coverage",
]

logger = logging.getLogger(__name__)

#: The schema of the coverage files this library writes and reads.
COVERAGE_SCHEMA = "openodd.coverage/1"
#: Read as :data:`COVERAGE_SCHEMA`: the same format under its earlier name.
COVERAGE_SCHEMAS = (COVERAGE_SCHEMA, "autoware_carla_scenario.coverage/1")
COVERAGE_FILE_GLOB = "*_coverage.json"

MergedEntry = _core.MergedEntry


@dataclass
class OddExposure:
    """How long the runs of one ODD spent inside and outside it."""

    name: str
    text: str = ""
    runs: int = 0
    #: Runs that left the ODD at least once.
    runs_outside: int = 0
    #: Inside, inside only because values were missing, and outside.
    ticks: dict[str, int] = field(default_factory=dict)
    seconds: dict[str, float] = field(default_factory=dict)
    #: Module -> ticks it failed, and ticks its verdict rested on missing values.
    module_ticks: dict[str, dict[str, int]] = field(default_factory=dict)
    unmeasured: list[str] = field(default_factory=list)
    #: The git sources the ODD was read from.
    sources: list[dict[str, Any]] = field(default_factory=list)
    #: The revisions of the ODD the runs measured (more than one: different ODDs).
    revisions: list[str] = field(default_factory=list)
    excursions: list[dict[str, Any]] = field(default_factory=list)

    def to_dict(self) -> dict[str, Any]:
        return {k: getattr(self, k) for k in self.__dataclass_fields__}


@dataclass
class CaseRun:
    """A run that recorded the case it ran.

    Attributes:
        document: The coverage file's index in the merge.
        scenario: The run's scenario.
        values: Parameter -> the value it ran with.
        buckets: Parameter -> the bucket drawn for it.
        chosen_for: ``"instability"`` (a flaky search chose it) or
            ``"coverage"`` (the sampler's own policy did).
        predicted: The flakiness predicted when it was chosen for its
            instability (a :class:`~.flaky.FlakyMetrics` as a dict).
        space: The normalized space it was chosen in, as written.
        passed: Its outcome, if recorded.
        assessed: Its flakiness over all the runs of its scenario and space.
    """

    document: int
    scenario: str
    values: dict[str, Any]
    buckets: dict[str, Any]
    chosen_for: str
    predicted: Optional[dict[str, Any]]
    space: Optional[list[dict[str, Any]]]
    passed: Optional[bool]
    assessed: Optional[dict[str, Any]]


class CoverageReport:
    """Coverage merged over a number of runs."""

    def __init__(self, core: Any) -> None:
        self._core = core

    @property
    def runs(self) -> int:
        return self._core.runs

    @property
    def scenarios(self) -> dict[str, int]:
        return self._core.scenarios

    @property
    def entries(self) -> list[MergedEntry]:
        return self._core.entries

    @property
    def odds(self) -> dict[str, OddExposure]:
        """ODD name -> how the runs measured against it fared."""
        return {name: OddExposure(**raw) for name, raw in json.loads(self._core.odds_json()).items()}

    @property
    def cases(self) -> list[CaseRun]:
        """The runs that recorded the case they ran, in the order merged."""
        return [CaseRun(**raw) for raw in json.loads(self._core.cases_json())]

    def entry(self, name: str) -> Optional[MergedEntry]:
        return next((e for e in self.entries if e.name == name), None)

    def groups(self) -> list[str]:
        return self._core.groups()

    def group_grade(self, group: str) -> float:
        return self._core.group_grade(group)

    def to_dict(self) -> dict[str, Any]:
        return json.loads(self._core.to_json())

    def to_markdown(self, *, max_holes: int = 20) -> str:
        return self._core.to_markdown(max_holes)


def merge_coverage(documents: Sequence[dict[str, Any]]) -> CoverageReport:
    """Merge coverage documents (the contents of coverage files) into a report."""
    report = CoverageReport(_core.merge_coverage(json.dumps(list(documents))))
    if report.runs == 0:
        logger.warning("coverage: nothing to merge")
    return report


def find_coverage_files(paths: Iterable[Path]) -> list[Path]:
    """Coverage files named by *paths*: files as given, directories searched."""
    found: list[Path] = []
    for path in map(Path, paths):
        if path.is_dir():
            found.extend(sorted(path.rglob(COVERAGE_FILE_GLOB)))
        elif path.is_file():
            found.append(path)
        else:
            logger.warning("coverage: %s does not exist", path)
    return found


def load_coverage_files(paths: Iterable[Path]) -> list[tuple[Path, dict[str, Any]]]:
    """The coverage files under *paths*, read; others are skipped with a warning."""
    documents: list[tuple[Path, dict[str, Any]]] = []
    for path in find_coverage_files(paths):
        try:
            doc = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            logger.warning("coverage: skipping unreadable %s", path, exc_info=True)
            continue
        if not isinstance(doc, dict) or doc.get("schema") not in COVERAGE_SCHEMAS:
            logger.warning("coverage: skipping %s: not a coverage file", path)
            continue
        documents.append((path, doc))
    return documents


def load_and_merge(paths: Iterable[Path]) -> CoverageReport:
    """Find the coverage files under *paths* and merge them."""
    return merge_coverage([doc for _, doc in load_coverage_files(paths)])


def main(argv: Optional[Sequence[str]] = None, *, prog: str = "openodd-coverage") -> int:
    """``openodd-coverage``: merge coverage files into a report."""
    parser = argparse.ArgumentParser(
        prog=prog,
        description="Merge *_coverage.json files and report ODD and scenario coverage: "
        "grade per item, and the buckets no run hit.",
    )
    parser.add_argument("paths", nargs="+", type=Path, help="coverage files, or directories to search")
    parser.add_argument("--json", type=Path, help="also write the merged report as JSON here")
    parser.add_argument("--markdown", type=Path, help="write the Markdown report here instead of stdout")
    parser.add_argument("--max-holes", type=int, default=20, help="holes listed per item (default: 20)")
    parser.add_argument(
        "--export-cod",
        type=Path,
        metavar="DIR",
        help="also write each run's ODD samples as an ASAM OpenODD COD table, "
        "with its manifest and taxonomy, into DIR",
    )
    args = parser.parse_args(argv)
    logging.basicConfig(level=logging.WARNING, format="%(levelname)s: %(message)s")
    documents = load_coverage_files(args.paths)
    report = merge_coverage([doc for _, doc in documents])
    if args.export_cod is not None:
        from .cod import export_cods  # noqa: PLC0415 - cod imports nothing of the report

        exported = export_cods(documents, args.export_cod)
        print(f"Exported {len(exported)} COD table(s) to {args.export_cod}", file=sys.stderr)  # noqa: T201
        if not exported:
            return 1  # asked for, and nothing had samples to export
    if report.runs == 0:
        print("No coverage files found.", file=sys.stderr)  # noqa: T201
        return 1
    markdown = report.to_markdown(max_holes=args.max_holes)
    if args.markdown is not None:
        args.markdown.parent.mkdir(parents=True, exist_ok=True)
        args.markdown.write_text(markdown, encoding="utf-8")
    else:
        sys.stdout.write(markdown)
    if args.json is not None:
        args.json.parent.mkdir(parents=True, exist_ok=True)
        args.json.write_text(json.dumps(report.to_dict(), indent=2), encoding="utf-8")
    return 0


if __name__ == "__main__":  # pragma: no cover
    sys.exit(main())
