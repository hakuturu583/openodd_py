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

What measures the attributes -- a simulator, a recorded log -- is the
application's :class:`Backend`.
"""

from ._core import GitSourceError, OpenOddError, UnitError, convert, normalize_unit
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
    GitSource,
    load_odd_archive,
    load_odd_binding,
    load_odd_file,
    load_openodd,
    read_archive_manifest,
)
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

__all__ = [
    "ABOVE_RANGE",
    "BELOW_RANGE",
    "COVER_MEASURES",
    "HOLDS",
    "INACTIVE",
    "UNDECIDED",
    "Backend",
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
]
