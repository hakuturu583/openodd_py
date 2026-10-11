"""Cases for the differential test: OpenODD documents, bindings, and values.

Each case is a set of files (one of them a binding file, the entry) and the
contexts to sample. Fixed cases come from the reference implementation's own
tests (the standard's example, its reader regressions); the rest are drawn at
random: taxonomies of numbers, categoricals, booleans, categoricals defined by
ranges, references; modules with AND/OR sections nested one level, labels,
inactive modules, ``unknown``.
"""

from __future__ import annotations

import random
from typing import Any

PROBE = "difftest_probes:read__{}"

SPEC_TAXONOMY = """
TAXONOMY:
    environment_conditions:
        rainfall_rate: float precipitation_rate
        rainfall_level:
            no_rain:
                rainfall_rate: "< 0.1 mm/h"
            light_rain:
                rainfall_rate: "[0.1 .. 2.5] mm/h"
            moderate_rain:
                rainfall_rate: "[2.5 .. 7.6] mm/h"
            heavy_rain:
                rainfall_rate: "> 7.6 mm/h"
        wind_speed: float velocity
        is_dangerous_wind:
            true:
                wind_speed: "> 50 km/h"
            false:
                wind_speed: "<= 50 km/h"
    scenery:
        road_type: [town_local, dead_end, town_expressway, expressway]
        current_road: road_type
        lane_count: integer count
        is_mixed_zone: boolean
        record_of_categoricals:
            surface: [dry, wet]
            marking: [solid, dashed]
    connectivity:
        downlink_latency: float time
        downlink_throughput: float bandwidth
    vehicle: reusable_pose
"""

SPEC_MODULES = """
IMPORT:
    - taxonomy.yml
ODD:
    odd1:
        TITLE: The baseline ODD
        INCLUDE_AND:
            low_speed_roads: true
            good_connectivity: true
        EXCLUDE_OR:
            bad_weather: true
MODULES:
    low_speed_roads:
        TITLE: Low speed traffic conditions
        INCLUDE_AND:
            road_type:
                - town_local
                - dead_end
            lane_count: "< 3"
            OR:
                rainfall_rate: "<= 2 mm/h"
                is_mixed_zone: false
    bad_weather_1:
        LABEL: bad_weather
        INCLUDE_OR:
            rainfall_level: ">= heavy_rain"
            is_dangerous_wind: true
    bad_weather_2:
        LABELS: bad_weather
        ACTIVE: "false"
        INCLUDE_AND:
            road_type: expressway
    good_connectivity:
        INCLUDE_AND:
            downlink_latency: "< 10 msec"
            downlink_throughput: "> 1 Mbps"
    required_data:
        EXCLUDE_OR:
            wind_speed: unknown
"""

SPEC_BINDING = """openodd: [odd.yml]
name: spec_example
text: The standard's example
probes:
  road_type: {probe: '%(p)s'}
  rainfall_rate: {probe: '%(r)s', unit: mm/h}
  wind_speed: {probe: '%(w)s', unit: m/s}
  lane_count: {probe: '%(l)s', values: [1, 2, 3]}
  is_mixed_zone: {probe: '%(m)s'}
  downlink_latency: {probe: '%(d)s', unit: ms}
  downlink_throughput: {probe: '%(t)s', unit: Mbps}
situations:
  bad_weather_1: {target: 2}
""" % {
    "p": PROBE.format("scenery.road_type"),
    "r": PROBE.format("environment_conditions.rainfall_rate"),
    "w": PROBE.format("environment_conditions.wind_speed"),
    "l": PROBE.format("scenery.lane_count"),
    "m": PROBE.format("scenery.is_mixed_zone"),
    "d": PROBE.format("connectivity.downlink_latency"),
    "t": PROBE.format("connectivity.downlink_throughput"),
}

REVIEW_TAXONOMY = """
TAXONOMY:
    rain_rate: float precipitation_rate
    rain_level:
        dry:
            rain_rate: "< 1 mm/h"
        wet:
            rain_rate: ">= 1 mm/h"
    rainfall: [none, light, heavy]
    wipers: [off, intermittent, continuous]
    country: [DE, NO, SE]
    road_type: [urban, rural]
    ego:
        state:
            road_type: [urban, rural]
    other: rain_level
    s: float velocity
    t: s
    flag: boolean
"""

REVIEW_MODULES = [
    "    m:\n        INCLUDE_AND:\n            rainfall: none\n",
    "    m:\n        INCLUDE_AND:\n            wipers: off\n            country: NO\n",
    "    m:\n        INCLUDE_AND:\n            state.road_type: [urban]\n",
    "    m:\n        INCLUDE_AND:\n            METADATA: {k: v}\n            flag: true\n",
    '    m:\n        INCLUDE_AND:\n            other: "< wet"\n            t: "> 5 km/h"\n',
    "    m:\n        EXCLUDE_OR:\n            rain_rate: unknown\n            rain_level: wet\n",
    '    m:\n        INCLUDE_OR:\n            s: "[10 .. 20] m/s"\n            AND:\n                flag: false\n                rainfall: [light, heavy]\n',
    "    m:\n        INCLUDE_AND:\n            flag: true\n            flag: false\n",
    '    m:\n        INCLUDE_AND:\n            rain_level: "> wet"\n',
    "    m:\n        INCLUDE_AND:\n            road_type: urban\n",
]


def _binding(concepts: list[str], extra: str = "", openodd: str = "odd.yml", name: str = "n") -> str:
    lines = [f"openodd: [{openodd}]", f"name: {name}", "probes:"]
    lines += [f"  {c}: {{probe: '{PROBE.format(c)}'{extra}}}" for c in concepts]
    return "\n".join(lines) + "\n"


def fixed_cases() -> list[dict[str, Any]]:
    cases = [{
        "id": "spec_example",
        "files": {"taxonomy.yml": SPEC_TAXONOMY, "odd.yml": SPEC_MODULES, "urban.binding.yaml": SPEC_BINDING},
        "entry": "urban.binding.yaml",
        "concepts": {
            "scenery.road_type": ["town_local", "dead_end", "town_expressway", "expressway", None],
            "environment_conditions.rainfall_rate": [0.0, 0.05, 0.1, 2.0, 2.5, 5.0, 7.6, 8.0, None],
            "environment_conditions.wind_speed": [0.0, 13.0, 13.8889, 14.0, 20.0, None],
            "scenery.lane_count": [1, 2, 3, 4, None],
            "scenery.is_mixed_zone": [True, False, None],
            "connectivity.downlink_latency": [1.0, 9.9, 10.0, 50.0, None],
            "connectivity.downlink_throughput": [0.5, 1.0, 1.5, None],
        },
    }]
    review = {
        "rain_rate": [0.0, 0.5, 1.0, 3.0, None],
        "rainfall": ["none", "light", "heavy", None],
        "wipers": ["off", "intermittent", "continuous", None],
        "country": ["DE", "NO", "SE", None],
        "road_type": ["urban", "rural", None],
        "ego.state.road_type": ["urban", "rural", None],
        "s": [0.0, 10.0, 15.0, 20.0, 25.0, None],
        "flag": [True, False, None],
    }
    for i, modules in enumerate(REVIEW_MODULES):
        odd = REVIEW_TAXONOMY + "MODULES:\n" + modules
        cases.append({
            "id": f"review_{i}",
            "files": {"odd.yml": odd, "b.yaml": _binding(list(review)) + "  s:\n    unit: m/s\n".replace("  s:\n    unit: m/s\n", "")},
            "entry": "b.yaml",
            "concepts": review,
        })
    return cases


# ---------------------------------------------------------------------------
# Random cases
# ---------------------------------------------------------------------------

UNITS = {"velocity": ["m/s", "km/h", "mph"], "length": ["m", "km", "ft"], "time": ["s", "ms", "min"]}


def random_case(seed: int) -> dict[str, Any]:
    rng = random.Random(seed)
    concepts: dict[str, dict[str, Any]] = {}
    taxonomy: list[str] = ["TAXONOMY:"]
    groups = ["env", "scene", "dyn"]
    for g in groups:
        taxonomy.append(f"    {g}:")
        for j in range(rng.randint(1, 3)):
            name = f"{g}_{j}"
            kind = rng.choice(["number", "number", "categorical", "boolean"])
            if kind == "number":
                unit_type = rng.choice(list(UNITS))
                taxonomy.append(f"        {name}: float {unit_type}")
                unit = rng.choice(UNITS[unit_type])
                concepts[f"{g}.{name}"] = {"kind": "number", "unit_type": unit_type, "unit": unit}
            elif kind == "categorical":
                literals = [f"{name}_l{k}" for k in range(rng.randint(2, 4))]
                taxonomy.append(f"        {name}: [{', '.join(literals)}]")
                concepts[f"{g}.{name}"] = {"kind": "categorical", "literals": literals}
            else:
                taxonomy.append(f"        {name}: boolean")
                concepts[f"{g}.{name}"] = {"kind": "boolean"}
    numbers = [c for c, d in concepts.items() if d["kind"] == "number"]
    if numbers and rng.random() < 0.7:
        # A categorical defined by ranges on a number.
        target = rng.choice(numbers)
        unit = rng.choice(UNITS[concepts[target]["unit_type"]])
        a, b = sorted(rng.sample(range(1, 60), 2))
        tail = target.split(".")[-1]
        taxonomy += [
            "    derived_level:",
            "        low:",
            f'            {tail}: "< {a} {unit}"',
            "        mid:",
            f'            {tail}: "[{a} .. {b}] {unit}"',
            "        high:",
            f'            {tail}: "> {b} {unit}"',
        ]
        concepts["derived_level"] = {"kind": "derived", "literals": ["low", "mid", "high"]}
    module_names = [f"m{i}" for i in range(rng.randint(1, 5))]

    def expression(concept: str) -> str:
        d = concepts[concept]
        tail = concept.split(".")[-1]
        if d["kind"] == "number":
            unit = rng.choice(UNITS[d["unit_type"]])
            x, y = sorted(round(rng.uniform(0, 80), 1) for _ in range(2))
            form = rng.choice(["range", "lt", "le", "gt", "ge", "eq", "unknown"])
            text = {
                "range": f'"[{x} .. {y}] {unit}"', "lt": f'"< {x} {unit}"', "le": f'"<= {x} {unit}"',
                "gt": f'"> {x} {unit}"', "ge": f'">= {x} {unit}"', "eq": f'"{x} {unit}"', "unknown": "unknown",
            }[form]
            return f"{tail}: {text}"
        if d["kind"] == "boolean":
            return f"{tail}: {rng.choice(['true', 'false', 'unknown'])}"
        if d["kind"] == "derived":
            op = rng.choice(["<", "<=", ">", ">=", "list", "eq"])
            if op == "list":
                return f"{tail}: [{', '.join(rng.sample(d['literals'], 2))}]"
            if op == "eq":
                return f"{tail}: {rng.choice(d['literals'])}"
            lit = {"<": "mid", "<=": rng.choice(d["literals"]), ">": "mid", ">=": rng.choice(d["literals"])}[op]
            return f'{tail}: "{op} {lit}"'
        choice = rng.random()
        if choice < 0.5:
            return f"{tail}: [{', '.join(rng.sample(d['literals'], rng.randint(1, len(d['literals']))))}]"
        return f"{tail}: {rng.choice(d['literals'])}"

    modules: list[str] = ["MODULES:"]
    labels: dict[str, str] = {}
    for i, name in enumerate(module_names):
        modules.append(f"    {name}:")
        if rng.random() < 0.3:
            label = rng.choice(["lab_a", "lab_b"])
            labels[name] = label
            modules.append(f"        LABEL: {label}")
        if rng.random() < 0.15:
            modules.append("        ACTIVE: false")
        sections = rng.choice([["INCLUDE_AND"], ["INCLUDE_OR"], ["EXCLUDE_OR"], ["EXCLUDE_AND"], ["INCLUDE_AND", "EXCLUDE_OR"]])
        for section in sections:
            modules.append(f"        {section}:")
            used = rng.sample(list(concepts), min(len(concepts), rng.randint(1, 3)))
            for c in used:
                modules.append(f"            {expression(c)}")
            later = module_names[i + 1:]
            if later and rng.random() < 0.4:
                ref = rng.choice(later + sorted(set(labels.values())) if labels else later)
                if ref not in [n for n in module_names[: i + 1]]:
                    modules.append(f"            {ref}: {rng.choice(['true', 'false'])}")
            if rng.random() < 0.3:
                nest = "OR" if section.endswith("AND") else "AND"
                modules.append(f"            {nest}:")
                for c in rng.sample(list(concepts), min(len(concepts), 2)):
                    modules.append(f"                {expression(c)}")
    # Units for the probes: some given, some left to the conditions.
    binding = [f"openodd: [odd.yml]", f"name: random_{seed}", "probes:"]
    for c, d in concepts.items():
        if d["kind"] == "derived":
            continue
        tail = c.split(".")[-1]
        extra = f", unit: {d['unit']}" if d["kind"] == "number" else ""
        if d["kind"] == "number" and rng.random() < 0.3:
            extra += f", buckets: [0, 20, 40, 80]"
        binding.append(f"  {tail}: {{probe: '{PROBE.format(c)}'{extra}}}")
    if module_names and rng.random() < 0.3:
        binding += ["situations:", f"  {rng.choice(module_names)}: {{target: 1}}"]
    samples: dict[str, list[Any]] = {}
    for c, d in concepts.items():
        if d["kind"] == "number":
            samples[c] = [None, 0.0, 5.0, 12.5, 20.0, 33.3, 40.0, 60.0, 79.9, 100.0]
        elif d["kind"] == "categorical":
            samples[c] = [None, *d["literals"], "zzz"]
        elif d["kind"] == "boolean":
            samples[c] = [None, True, False]
    return {
        "id": f"random_{seed}",
        "files": {"odd.yml": "\n".join(taxonomy + modules) + "\n", "b.yaml": "\n".join(binding) + "\n"},
        "entry": "b.yaml",
        "concepts": samples,
    }


def contexts(case: dict[str, Any], n: int = 120, seed: int = 0) -> list[dict[str, Any]]:
    rng = random.Random(f"{case['id']}/{seed}")
    concepts = case["concepts"]
    return [{c: rng.choice(values) for c, values in concepts.items()} for _ in range(n)]


def all_cases(randoms: int = 300) -> list[dict[str, Any]]:
    return fixed_cases() + [random_case(seed) for seed in range(randoms)]
