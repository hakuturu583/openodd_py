# Writing an ODD in Python

An ODD is read from OpenODD YAML and a binding file (`load_odd_binding`), or
written in Python. Both build the same `OddDefinition`, so they are judged,
covered, sampled and reported the same way. A Python ODD has no files: its
attributes carry their probes, so it needs no binding, and its version is its
**structure** (the digest of the ODD as built; see
[versioning.md](versioning.md)).

## An ODD is a function

An ODD is a function that takes nothing and returns an `OddDefinition`. A
probe is any callable taking the backend's *context* (a simulator's world, a
log row) and returning the value, or `None` when it is missing:

```python
from openodd import OddAttribute, OddDefinition, OddModule, module_holds, register_odd

INTENSITY = ["none", "light", "moderate", "heavy"]


def speed_limit_kph(context) -> float | None:
    return context.get("speed_limit")


def location(context) -> str | None:
    return context.get("location")


def rain(context) -> str | None:
    return context.get("rain")


def urban_odd() -> OddDefinition:
    where = OddAttribute("scenery.location", location, values=["urban", "nonurban", "private"])
    limit = OddAttribute(
        "scenery.speed_limit", speed_limit_kph, unit="km/h",
        buckets=[0, 30, 40, 50, 60, 80, 100],
    )
    weather = OddAttribute("environment.rain", rain, values=INTENSITY)
    return OddDefinition(
        "urban",
        [where, limit, weather],
        [
            OddModule(
                "urban_roads",
                include_and=[where.is_in(["urban"]), limit.between(0, 60)],
                text="Urban roads up to 60 km/h",
            ),
            OddModule("weather", exclude_or=[weather.is_in(["heavy"])], labels=["fair"]),
            OddModule("root", include_and=[module_holds("urban_roads"), module_holds("fair")]),
        ],
        roots=["root"],
        text="Urban roads, fair weather",
    )


register_odd("urban", urban_odd)
```

A backend can hand its own probes to a Python ODD as well: they are plain
callables (`backend.probes["speed_kph"][0]`).

### Attributes

`OddAttribute(name, probe, *, unit, range, every, buckets, values, text,
target, cover_by, min_stay)`. Give at most one of `values`, `buckets` and
`range` (with `every`). An attribute with none of them is **monitored but not
covered**: conditions may test it, and the report lists it so the blind spot
is visible. Its cover item is named `"odd." + name`.

### Conditions

| Method | Holds when |
|---|---|
| `attr.is_in(values)` | the value is one of `values` (compared by label: an enum by name, a bool as `true`/`false`) |
| `attr.equals(value)` | the value equals `value` |
| `attr.between(low, high)` | `low <= value <= high` |
| `attr.at_least(x)` / `attr.at_most(x)` | `value >= x` / `value <= x` |
| `attr.greater_than(x)` / `attr.less_than(x)` | `value > x` / `value < x` |
| `attr.is_unknown()` | the value is missing |
| `all_of([...])` / `any_of([...])` | all of / any of the conditions hold |
| `module_holds(name, holds=True)` | the module (or label) named `name` holds, or does not |

Numbers are in the attribute's `unit`.

### Modules

`OddModule(name, *, include_and, include_or, exclude_and, exclude_or, labels,
active, text, situation, target, cover_by, min_stay, metadata)`. Give at most
one of `include_and` / `include_or`, and at most one of `exclude_and` /
`exclude_or`. Modules follow ISO 34503's "default" definition mode, as OpenODD
requires: whatever no module rules out is inside the ODD.

### Situations

Attribute coverage counts each attribute on its own. A module already says a
combination, so a module can be **covered as a situation**:

```python
OddModule(
    "three_lane_cruise",
    include_and=[lanes.equals(3), speed.between(55, 65)],
    situation=True,
    cover_by="meters", target=200, min_stay=3.0,   # 200 m, counting stays of 3 s or more
)
```

Every step, the module's verdict is a sample of the item
`odd.situation.three_lane_cruise`, which has one bucket, `holds`. A situation
is what the runs should drive, not a bound of the ODD: with `roots` left out,
a situation nothing refers to is not a root candidate.

## Picking an ODD by name

An application lets its user name the ODD by a string -- a config key, a
command-line argument -- and reads it with `resolve_odd`:

```python
odd = openodd.resolve_odd(spec, backend=my_backend)
```

| `spec` | ODD |
|---|---|
| `None`, `""`, `default` | The bundled `default` ODD (`bundled_odd()`), measured by `backend` |
| another name in `BUNDLED_ODDS` | That bundled ODD |
| a registered name | The ODD `register_odd(name, builder)` registered, in code or through an entry point |
| `path/to/urban.yaml` | A binding file, or an OpenODD document on its own (`load_odd_file`), measured by `backend` |
| `package.module:function` | A builder to import and call |
| an `OddDefinition` | Itself |

`backend` (the default backend when left out) measures what is read from
files; a Python ODD brings its probes. `odd_builder(spec)` returns the Python
builder a spec names, or `None` for a bundled ODD or a file -- for an
application that checks Python ODDs before it runs them. `odd_names()` lists
the names an ODD can be picked by.

### Entry points

A package that ships its ODD registers it through the `openodd.odds` entry
point group. The entry point names a function that takes nothing and calls
`register_odd`:

```toml
[project.entry-points."openodd.odds"]
my_package = "my_package.odds:register"
```

```python
def register() -> None:
    register_odd("urban", urban_odd)
```

The entry points are loaded the first time a name is looked up. One that
fails to load is logged and skipped. An application that advertises a group
of its own adds it with `add_entry_point_group("my_app.odds")`; its entry
points follow the same convention.
