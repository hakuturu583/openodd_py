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

A run that recorded its outcome also counts in the report's flakiness per ODD
cell (:attr:`CoverageReport.cell_flakiness`, a :class:`CellFlakiness` per
scenario, environment and attribute list): for every cell -- one bucket of
each of the ODD's attributes -- the runs that drove through it, how many of
them failed, and how many failed while in it.
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
    "CELL_BAND",
    "CELL_MIN_RUNS",
    "COVERAGE_FILE_GLOB",
    "COVERAGE_SCHEMA",
    "COVERAGE_SCHEMAS",
    "CaseRun",
    "CellFlakiness",
    "CellRow",
    "CoverageReport",
    "MergedEntry",
    "OddExposure",
    "find_coverage_files",
    "load_and_merge",
    "load_coverage_files",
    "main",
    "merge_coverage",
    "wilson_interval",
]

logger = logging.getLogger(__name__)

#: The schema of the coverage files this library writes and reads.
COVERAGE_SCHEMA = "openodd.coverage/1"
#: Read as :data:`COVERAGE_SCHEMA`: the same format under its earlier name.
COVERAGE_SCHEMAS = (COVERAGE_SCHEMA, "autoware_carla_scenario.coverage/1")
COVERAGE_FILE_GLOB = "*_coverage.json"

MergedEntry = _core.MergedEntry

#: The fewest runs that must have visited a cell for it to be called flaky
#: (or always failing), unless asked otherwise.
CELL_MIN_RUNS: int = _core.CELL_MIN_RUNS
#: A cell is flaky when the 95% interval of its failure rate lies inside this
#: band, unless asked otherwise: both outcomes then happen at least 5% of the
#: time, at 95% confidence.
CELL_BAND: tuple[float, float] = tuple(_core.CELL_BAND)  # type: ignore[assignment]


def wilson_interval(failed: int, runs: int) -> tuple[float, float]:
    """The 95% Wilson score interval of *failed* failures in *runs* runs
    (``(0.0, 1.0)`` for no runs)."""
    low, high = _core.wilson_interval(int(failed), int(runs))
    return float(low), float(high)


def _check_thresholds(min_runs: int, band: tuple[float, float]) -> tuple[float, float]:
    low, high = (float(b) for b in band)
    if min_runs < 1:
        raise ValueError(f"min_runs must be at least 1, not {min_runs}")
    if not 0.0 <= low < high <= 1.0:
        raise ValueError(f"band must be (low, high) with 0 <= low < high <= 1, not {band}")
    return low, high


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


@dataclass(frozen=True)
class CellRow:
    """One ODD cell, over the runs of a :class:`CellFlakiness` group.

    Attributes:
        labels: One bucket label per attribute of the group.
        runs_visited: Runs (with an outcome) that spent any step in it.
        runs_failed: Of those, how many failed.
        fail_rate: ``runs_failed / runs_visited``.
        ci_low: The lower end of its 95% Wilson interval.
        ci_high: The upper end.
        fail_at_cell: Failed runs that were in this cell when they failed.
        seconds: Seconds the runs spent in it.
        meters: Metres they drove in it.
    """

    labels: tuple[str, ...]
    runs_visited: int
    runs_failed: int
    fail_rate: float
    ci_low: float
    ci_high: float
    fail_at_cell: int
    seconds: float
    meters: float

    @classmethod
    def from_row(cls, row: Sequence[Any]) -> "CellRow":
        labels, visited, failed, rate, low, high, at, seconds, meters = row
        return cls(tuple(labels), int(visited), int(failed), float(rate), float(low),
                   float(high), int(at), float(seconds), float(meters))

    def to_row(self) -> list[Any]:
        """``[[labels...], runs_visited, runs_failed, fail_rate, ci_low, ci_high,
        fail_at_cell, seconds, meters]``."""
        return [list(self.labels), self.runs_visited, self.runs_failed, self.fail_rate,
                self.ci_low, self.ci_high, self.fail_at_cell, self.seconds, self.meters]

    def status(self, min_runs: int = CELL_MIN_RUNS,
               band: tuple[float, float] = CELL_BAND) -> str:
        """``"flaky"``: at least *min_runs* runs visited it, some failed and some
        passed, and the 95% interval lies inside *band*; ``"failing"``: every one
        of them failed (a bug, not flakiness); ``"mixed"``: both outcomes, but
        the interval reaches outside *band*; ``"passing"``: none failed;
        ``"too_few"``: fewer than *min_runs* runs."""
        low, high = _check_thresholds(min_runs, band)
        return str(_core.cell_status(self.runs_visited, self.runs_failed, int(min_runs), low, high))

    def is_flaky(self, min_runs: int = CELL_MIN_RUNS,
                 band: tuple[float, float] = CELL_BAND) -> bool:
        return self.status(min_runs, band) == "flaky"


@dataclass
class CellFlakiness:
    """The runs of one scenario and environment, per cell of one attribute list.

    Attributes:
        scenario: The runs' scenario.
        environment: Their environment (``outcome.environment``); ``None``
            for runs that recorded none, which are pooled only with each other.
        attributes: The attributes a cell's labels are of, in order.
        runs: Runs pooled: those with an outcome and the ODD's cells.
        runs_failed: Of those, how many failed.
        failures_located: Failed runs that recorded the cell they failed in.
        documents: The coverage files pooled (their indices in the merge).
        rows: The cells visited, flakiest first, then by labels.
    """

    scenario: str
    environment: Optional[str]
    attributes: list[str]
    runs: int
    runs_failed: int
    failures_located: int
    documents: list[int]
    rows: list[CellRow]

    @classmethod
    def from_dict(cls, raw: dict[str, Any]) -> "CellFlakiness":
        return cls(
            scenario=raw["scenario"],
            environment=raw["environment"],
            attributes=list(raw["attributes"]),
            runs=int(raw["runs"]),
            runs_failed=int(raw["runs_failed"]),
            failures_located=int(raw["failures_located"]),
            documents=list(raw["documents"]),
            rows=[CellRow.from_row(r) for r in raw["rows"]],
        )

    def to_dict(self) -> dict[str, Any]:
        """``{"scenario", "environment", "attributes": [...], "runs", "runs_failed",
        "failures_located", "documents", "rows": [[[labels...], runs_visited,
        runs_failed, fail_rate, ci_low, ci_high, fail_at_cell, seconds, meters], ...]}``."""
        return {
            "scenario": self.scenario,
            "environment": self.environment,
            "attributes": list(self.attributes),
            "runs": self.runs,
            "runs_failed": self.runs_failed,
            "failures_located": self.failures_located,
            "documents": list(self.documents),
            "rows": [r.to_row() for r in self.rows],
        }

    def cell(self, labels: Sequence[str]) -> Optional[CellRow]:
        """The cell with these labels, if any run visited it."""
        key = tuple(labels)
        return next((r for r in self.rows if r.labels == key), None)

    def flaky_cells(self, min_runs: int = CELL_MIN_RUNS,
                    band: tuple[float, float] = CELL_BAND) -> list[CellRow]:
        """The cells that are flaky (:meth:`CellRow.status`), flakiest first."""
        return [r for r in self.rows if r.is_flaky(min_runs, band)]

    def failing_cells(self, min_runs: int = CELL_MIN_RUNS) -> list[CellRow]:
        """The cells every one of at least *min_runs* runs failed in."""
        return [r for r in self.rows if r.status(min_runs, (0.0, 1.0)) == "failing"]


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

    @property
    def cell_flakiness(self) -> list[CellFlakiness]:
        """The runs' outcomes per ODD cell: one :class:`CellFlakiness` per
        scenario, environment and attribute list, in the order merged. Empty
        when no run recorded both an outcome and the ODD's cells."""
        return [CellFlakiness.from_dict(raw) for raw in json.loads(self._core.cell_flakiness_json())]

    def flaky_cells(self, min_runs: int = CELL_MIN_RUNS,
                    band: tuple[float, float] = CELL_BAND) -> list[tuple[CellFlakiness, CellRow]]:
        """Every flaky cell (:meth:`CellRow.status`), with its group."""
        return [(g, r) for g in self.cell_flakiness for r in g.flaky_cells(min_runs, band)]

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
    _warn_mismatched_cells(report.cell_flakiness)
    return report


def _warn_mismatched_cells(groups: Sequence[CellFlakiness]) -> None:
    """Warn of runs of one scenario and environment with different ODD attributes."""
    lists: dict[tuple[str, Optional[str]], list[CellFlakiness]] = {}
    for g in groups:
        lists.setdefault((g.scenario, g.environment), []).append(g)
    for (scenario, environment), apart in lists.items():
        if len(apart) > 1:
            logger.warning(
                "coverage: the runs of %s (environment %s) measured different ODD "
                "attributes; their cells are reported apart: %s",
                scenario, environment,
                "; ".join(f"documents {g.documents}: {g.attributes}" for g in apart),
            )


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
