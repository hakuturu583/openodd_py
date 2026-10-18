"""ASAM OpenODD 1.0 for Python, on a Rust core.

Read an ODD from OpenODD YAML and a binding file, judge values against it
with OpenODD's missing-value semantics, work out which buckets lie outside
it, and version it by the files it was read from::

    import openodd

    backend = openodd.Backend(probes={"speed_kph": (read_speed, "km/h")})
    odd = openodd.load_odd_binding("urban.yaml", backend=backend)
    verdict = odd.evaluate(odd.sample(context))
    odd.revision          # the digest of the files it was read from
    odd.archive()         # those files, as a deterministic .tar.gz

An ODD can be written in Python too, registered by name, and picked by a
string (:mod:`.registry`)::

    openodd.register_odd("urban", urban_odd)   # urban_odd() returns an OddDefinition
    odd = openodd.resolve_odd("urban")         # or "urban.yaml", "pkg.mod:fn", "default"

What measures the attributes -- a simulator, a recorded log -- is the
application's :class:`Backend`.
"""

from importlib import metadata as _metadata

from ._core import GitSourceError, OpenOddError, UnitError, Units, convert, normalize_unit
from .cells import MISSING, OddCells
from .backend import Backend, Probe, default_backend, import_callable, set_default_backend
from .items import (
    ABOVE_RANGE,
    BELOW_RANGE,
    COVER_MEASURES,
    HOLDS,
    CoverGroup,
    CoverItem,
    CrossItem,
    SamplingEvent,
    duplicates,
    event_name,
    value_label,
)
from .loader import (
    BUNDLED_ODDS,
    GitSource,
    bundled_odd,
    load_odd_archive,
    load_odd_binding,
    load_odd_file,
    load_openodd,
    read_archive_manifest,
)
from .cod import CodExport, export_cod, export_cods
from .collector import CROSS_SEPARATOR, CoverageCollector
from .report import (
    CELL_BAND,
    CELL_MIN_RUNS,
    COVERAGE_SCHEMA,
    CaseRun,
    CellFlakiness,
    CellRow,
    CoverageReport,
    MergedEntry,
    find_coverage_files,
    load_and_merge,
    load_coverage_files,
    merge_coverage,
    wilson_interval,
)
from . import view
from .view import CasePlot, CoverageView
from .testreport import Finding, ReportAnalysis, TestReport
from . import agent
from .sampler import OddDraws, OddKnob, OddSample, OddSampler, knobs_from_mapping
from .flaky import FlakinessModel, FlakyMetrics, FlakySearch, ParameterSpace
from .model import (
    INACTIVE,
    UNDECIDED,
    OddAttribute,
    OddCondition,
    OddDefinition,
    OddModule,
    OddVerdict,
    all_of,
    any_of,
    module_holds,
    read_probe,
)
from . import registry
from .registry import (
    DEFAULT_ODD,
    ENTRY_POINT_GROUP,
    add_entry_point_group,
    odd_builder,
    odd_names,
    register_odd,
    registered_odds,
    resolve_odd,
    unregister_odd,
)

try:
    __version__: str = _metadata.version("openodd-py")
except _metadata.PackageNotFoundError:  # a source tree, not installed
    __version__ = "0+unknown"

__all__ = [
    "DEFAULT_ODD",
    "ENTRY_POINT_GROUP",
    "add_entry_point_group",
    "odd_builder",
    "odd_names",
    "register_odd",
    "registered_odds",
    "registry",
    "resolve_odd",
    "unregister_odd",
    "Finding",
    "ReportAnalysis",
    "TestReport",
    "agent",
    "FlakinessModel",
    "FlakyMetrics",
    "FlakySearch",
    "ParameterSpace",
    "CROSS_SEPARATOR",
    "CodExport",
    "CasePlot",
    "CoverageView",
    "CoverageCollector",
    "MISSING",
    "OddCells",
    "__version__",
    "export_cod",
    "export_cods",
    "CELL_BAND",
    "CELL_MIN_RUNS",
    "COVERAGE_SCHEMA",
    "CaseRun",
    "CellFlakiness",
    "CellRow",
    "CoverageReport",
    "MergedEntry",
    "OddDraws",
    "OddKnob",
    "OddSample",
    "OddSampler",
    "find_coverage_files",
    "knobs_from_mapping",
    "load_and_merge",
    "load_coverage_files",
    "merge_coverage",
    "wilson_interval",
    "ABOVE_RANGE",
    "BELOW_RANGE",
    "COVER_MEASURES",
    "HOLDS",
    "INACTIVE",
    "UNDECIDED",
    "BUNDLED_ODDS",
    "Backend",
    "bundled_odd",
    "CoverGroup",
    "CoverItem",
    "CrossItem",
    "GitSource",
    "GitSourceError",
    "OddAttribute",
    "OddCondition",
    "OddDefinition",
    "OddModule",
    "OddVerdict",
    "OpenOddError",
    "Probe",
    "SamplingEvent",
    "UnitError",
    "Units",
    "all_of",
    "any_of",
    "convert",
    "default_backend",
    "duplicates",
    "event_name",
    "import_callable",
    "load_odd_archive",
    "load_odd_binding",
    "load_odd_file",
    "load_openodd",
    "module_holds",
    "normalize_unit",
    "read_archive_manifest",
    "read_probe",
    "set_default_backend",
    "value_label",
    "view",
]
