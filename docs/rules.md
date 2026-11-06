# Rules

A backend's knobs set what an ODD names: the rain, the sun. What those do to
the rest of a run -- the road's grip, how far drivers see -- is the backend's
own model. It may write it as rules: a YAML file that `openodd.Rules` reads,
which the Rust core evaluates. Each rule is a **conditional distribution**:
given some inputs, the distribution of a quantity. A run draws a value from it
at a quantile.

## Where the rules stand

The rules are a Bayesian network over the conditions a run is given and what
follows from them. They sit beside the ODD and its taxonomy, not in them:

| | Holds | Where |
|---|---|---|
| **Taxonomy** | The vocabulary: each condition's name and its value range, qualitative and orthogonal (rain intensity, by its rainfall rate) | An OpenODD file -- a shared one, such as tier4/taxonomy |
| **ODD** | Which values of that vocabulary a system is designed for | OpenODD modules over the taxonomy |
| **Rules: causes** | The inputs the rules are given: conditions of the taxonomy, in its vocabulary and value ranges | `inputs:` with a `concept` ([Inputs and the taxonomy](#inputs-and-the-taxonomy)) |
| **Rules: effects** | What the causes do -- the road's grip, how far drivers see -- and the simulator's own settings: never in the taxonomy | `quantities:` |
| **Rules: distributions** | How the effects are spread given the causes: an **assumption about how faithfully the simulator renders the conditions**, not part of the ODD | each part, with its `basis` ([The basis of a part](#the-basis-of-a-part)) |

Why so:

- **The taxonomy stays small and stable.** Causal links and distributions
  are not conditions a system is designed for; written into the taxonomy,
  they would swell its vocabulary and couple each of its terms to one
  simulator. Kept apart, a change of the taxonomy reaches the rules only
  where they name its concepts, and the rules change without touching it.
- **Every cause can be traced.** Since each input is a concept of the
  taxonomy, every condition a rule conditions on is one an ODD can name and
  restrict -- no "dark ODD" hides in the rules. A condition the taxonomy
  lacks is added to it, not invented in the rules.
- **Every assumption says what it rests on.** A distribution fitted to data
  measured in the real world and one set by someone's judgement are not
  equally strong; the `basis` of each part says which, the fit writes it
  ([Fitting to data](#fitting-to-data)), and a campaign's report lists the
  assumptions its runs rested on ([What a campaign assumed](#what-a-campaign-assumed)).

```python
import openodd

backend = openodd.Backend(
    rules=openodd.Rules.read("grip.yaml"),   # the file below
    rule_inputs=["water"],
)
backend.derived_range("tire_friction", {"water": 70.0})      # (0.57, 0.77)
backend.derived_at("tire_friction", {"water": 70.0}, 0.25)   # 0.62
backend.use_rules("measured_grip.yaml")   # a run's rules, over the backend's
backend.use_rules(None)                   # back to the backend's alone
```

## The rule file

```yaml
version: 2
quantities:
  tire_friction:
    parts:
      water:
        at: {water: [0, 50, 100]}          # the input, and the points written
        uniform:                           # the distribution at each point
          low:  [0.90, 0.65, 0.45]
          high: [1.10, 0.85, 0.65]
```

| Key | Meaning |
|---|---|
| `version` | `2` or `3` ([Inputs and the taxonomy](#inputs-and-the-taxonomy)) |
| `taxonomy` | Version 3: the OpenODD taxonomy the inputs are concepts of -- a path, URL or git source, as `extends` takes |
| `basis` | What the parts' values rest on ([The basis of a part](#the-basis-of-a-part)); on the file, a quantity or a part; required for every part from version 3 |
| `inputs` | Optional: name → `default` (the value read when none is given), `text` and `labels` (a categorical input's, in order); from version 3 also `concept` and `unit` |
| `quantities` | Name → a quantity: `parts`, `combine` and an optional `text` |
| `combine` | How the parts' values are joined: `min` (the default) or `max` |
| `parts` | Name → a part: `at`, one distribution, an optional `text`, and an optional `fixed` |
| `at` | Input → its points, increasing. The order of the inputs is the order of the values' nesting. Left out (or `{}`), the part reads no input ([A part that reads no input](#a-part-that-reads-no-input)) |
| `uniform` | `low`, `high` |
| `triangular` | `low`, `mode`, `high` |
| `normal` | `mean`, `sd`, truncated to `low`..`high` |
| `fixed` | What a fit to data holds as written ([Holding values](#holding-values)): `true`, or a list of `param`, `param@input=point,...`, `@input=point,...` |

- **Values.** A parameter is one number, the same everywhere, or one value per
  point. With several inputs it is nested lists, one level per input in `at`'s
  order: `at: {a: [0, 10], b: [0, 1]}` takes `[[at a=0 b=0, at a=0 b=1], [at
  a=10 b=0, at a=10 b=1]]`.
- **Between the points**, each parameter is interpolated linearly
  (multilinearly over several inputs). **Beyond them** it holds the outermost
  point's value. An input not given reads its `default`, and without one is
  an error.
- **A quantile.** A run draws `p` between 0 and 1 and takes the value with a
  share `p` of the distribution's mass below it (`Rules.quantile`). For a
  uniform distribution that is `low + p * (high - low)`.
- **Parts move together.** With several parts, each gives its value at the
  same `p`, and `combine` joins them: `min` takes the least. The parts behave
  as if one common cause moved them all (fog, rain and night all at their
  worse end, or all at their better end). So a quantity's range is the least
  of the parts' `low` to the least of their `high`, and the value at `p` is
  the least of the parts' values at `p`.
- `Rules.evaluate` gives `low`, `high`, `median` and `mean`; `Rules.range`
  `(low, high)`; `Rules.cdf` the share of the mass at or below a value;
  `Rules.distributions` each part's distribution at the inputs.

### A categorical input

An input may be categorical: the time of day, a rain intensity. Its labels
are the file's `labels` (in order), or, from version 3, its categorical
concept's literals. A part reading it is written at its labels, in that
order, and nothing lies between two of them; a number input beside it is
interpolated as usual:

```yaml
inputs:
  time_of_day: {labels: [day, dusk, night, dawn], default: day}
quantities:
  carla_sun_altitude:
    parts:
      time:
        at: {time_of_day: [day, dusk, night, dawn]}
        uniform: {low: [10, -6, -30, -6], high: [70, 0, -6, 0]}
```

It is given as a label (`rules.range("carla_sun_altitude", {"time_of_day":
"dusk"})`); its `default` is one. A label the part is not written at is an
error, not a value between two. A fit reads it as a label in its rows and
writes the part's labels back; a sampler's knob or effect reads the label the
case drew for the attribute it stands for. A categorical concept has no
`unit`. `Rules.input_labels` gives each categorical input's labels.

### A part that reads no input

A part with no `at` (or `at: {}`) is one distribution: a root of the
network, with no cause among the ODD's conditions. What a scenario sets up --
the gap to a vehicle about to cut in, how fast a pedestrian crosses -- is such
an effect when nothing in the domain conditions it yet:

```yaml
quantities:
  vehicle_ahead_gap:
    parts:
      staged:
        uniform: {low: 2, high: 30}       # one number per parameter
```

Its parameters are one number each (a list needs points to put it at). It
joins the other parts of its quantity as any part does, so a part that reads
no input beside one that does bounds the quantity everywhere (`min` of a
floor and the water's grip). A fit keeps it a root and writes `at: {}`. In a
file that extends another, a part without `at` keeps the points there
([How files merge](#extending-a-rule-file)); `at: {}` makes it read none.

## Inputs and the taxonomy

A rule's inputs are causes: the conditions a run is given. The taxonomy is
the shared vocabulary those conditions are named in, so from version 3 every
input a part reads stands for one of its concepts, and the rules name the
taxonomy they draw on. The quantities are the effects the rules work out --
the road's grip, the simulator's settings -- and stand for no concept: the
taxonomy holds vocabulary and value ranges, not what follows from them.

```yaml
version: 3
taxonomy: {git: https://github.com/tier4/taxonomy, rev: <commit>, path: taxonomy_openodd.yaml}
basis: {kind: expert, by: Safety Strategy, date: 2026-11-06}
inputs:
  rainfall_rate:
    concept: environment_conditions.environment_conditions.weather.rain.rainfall_rate
    unit: mm/h                         # the unit the points are written in
quantities:
  tire_friction:
    parts:
      rain:
        at: {rainfall_rate: [0, 2.5, 7.6]}
        uniform: {low: [0.9, 0.7, 0.5], high: [1.1, 0.85, 0.65]}
```

Reading a version 3 file refuses:

- an input a part reads that names no `concept` (a free input left out of
  `inputs` too): add the concept to the taxonomy, or make the input a
  quantity the rules work out;
- a `concept` the taxonomy does not declare, or one that is not a number (a
  part's points are numbers);
- a `unit` of another dimension than the concept's unit type;
- a quantity with a `concept`.

The taxonomy is read from where the file that names it says, like
`extends`; a file it extends may name it instead, and the nearest name wins.
`taxonomy: {bundled: default}` names the taxonomy openodd ships, the default
ODD's, read from the core's own copy (label `bundled:default`) with no file or
network access, in Python and in the browser alike. It stands in for
tier4/taxonomy as a subset of it (the same ids and definitions, for the
concepts openodd uses) until that can be named: converging then is changing
this reference to its git source.
It is one OpenODD file, read on its own: one that `IMPORT`s others is refused, since its imports would be read from the working directory and its SHA-256 would not cover them.
`Rules.version`, `Rules.taxonomy` (`(path or URL, sha256)`) and
`Rules.input_concepts` (input → `(concept, unit)`) say what was read.

Version 2 files are still read. Their inputs stand for nothing the rules can
check (`Rules.unclassified_inputs`), and reading one warns
(`UnclassifiedInputsWarning`). A version 2 file merged over a version 3 set --
a fit's, say -- keeps the set's version and its taxonomy.

## The basis of a part

A rule is not part of an ODD. It is an assumption about how faithfully a run
renders the conditions it is given -- what rain does to the grip in this
simulator -- and an assumption says what it rests on. `basis:` says so for a
part:

```yaml
basis: {kind: expert, by: Safety Strategy}           # the file's: parts that say nothing
quantities:
  tire_friction:
    basis: {kind: literature, reference: "ISO 8349"}  # its parts' that say nothing
    parts:
      rain:
        at: {rainfall_rate: [0, 2.5, 7.6]}
        uniform: {low: [0.9, 0.7, 0.5], high: [1.1, 0.85, 0.65]}
        basis:                                        # its own
          kind: fitted
          data: {file: drives_2026q3.csv, sha256: 3f9a..., rows: 18234}
          calibration: {in_band: 0.88, ks: 0.021}
          by: openodd-rules-fit
          date: 2026-11-06
```

| Field | |
|---|---|
| `kind` | `fitted` (estimated from data), `expert` (set by someone who knows) or `literature` (taken from a published source) |
| `data` | `file`, `sha256`, `rows`: the data a fit was made from |
| `calibration` | Numbers saying how well the part matches data ([How well the rules match](#how-well-the-rules-match)) |
| `by`, `date`, `reference`, `text` | Who set it, when, from where, and anything else |

A part's basis is its own, else its quantity's, else the file's. From
version 3 every part must have one; a version 2 part with none is
`unstated`. A merged file's `basis` replaces the one there, field by field
as `text` does. `Rules.basis(quantity, part)` gives it as a dict,
`Rules.bases()` every part's.

### What a campaign assumed

A run's coverage file records the rules its backend followed
(`Backend.active_rules`) when it ends:

```json
"rules": {"version": 3, "taxonomy": ["taxonomy.yaml", "9c1e..."],
          "files": [["weather_rules.yaml", "3195..."]],
          "parts": [{"quantity": "tire_friction", "part": "rain",
                     "basis": {"kind": "fitted", "data": {"rows": 18234}, ...}}]}
```

`merge_coverage` gathers them: `CoverageReport.assumptions` lists each part
with what it rests on and the runs that followed it so (a part whose basis
changed between runs is listed once per basis). The Markdown report and
`openodd-test-report` add a **Fidelity assumptions** section, flagging the
parts that rest on nothing stated.

## Knobs from the rules

A knob that writes a fixed table -- `rain: {none: 0, light: [1, 30], ...}` --
hides an assumption about the simulator in code: that light rain *is*
CARLA's precipitation 1 to 30. Written as a quantity of the rules instead,
the setting is an **effect** of the condition the sampler drew, with a
distribution and a `basis` like any other, and a fit can estimate it from
data pairing real conditions with the settings that reproduce them:

```yaml
version: 3
taxonomy: taxonomy.yaml
basis: {kind: expert}
inputs:
  rainfall_rate: {concept: weather.rain.rainfall_rate, unit: mm/h}
quantities:
  carla_precipitation:             # the simulator's setting: an effect
    parts:
      rain:
        at: {rainfall_rate: [0, 2.5, 7.6, 50]}
        uniform: {low: [0, 20, 45, 85], high: [5, 40, 75, 100]}
```

```python
knob = openodd.OddKnob("environment.precipitation", quantity="carla_precipitation", limits=(0, 100))
sampler = openodd.OddSampler(odd, knobs={"weather.rain.rainfall_rate": knob}, backend=backend)
```

The sampler draws the attribute (`weather.rain.rainfall_rate`, in its own
unit) as it would for any knob, and the knob writes the quantity's value at
it: read through the rule input that stands for the attribute (`concept:`;
an input of the attribute's name in version 2 rules), from the backend's
active rules, at a quantile of its own per case and knob -- the same for the
same seed. `limits` and `integer` apply to the value written; `values`,
`scale` and `offset` are not used. A knob in config takes `quantity:` too
(`knobs_from_mapping`), and `Backend.manifest()` writes it. The case keeps
the value written (`OddSample.quantities`, recorded as `case.quantities`), so
a [case plot](case_plot.md) can place the runs by it.

**Fitting a knob's quantity.** A setting's quantity is fitted like any
other ([Fitting to data](#fitting-to-data)), from rows that pair the real
condition with the setting that reproduces it -- the rainfall rate a rain
gauge measured, and the CARLA precipitation whose camera and LiDAR images
matched the drive's:

```python
rows = [{"rainfall_rate": 2.1, "carla_precipitation": 21.0}, ...]
fit = rules.fit(rows, data_files=["matched.csv"], by="sensor matching")
fit.write("rules_fitted.yaml", extends="rules.yaml")   # basis: fitted
```

The fitted setting then rests on data rather than on a table in code, and a
campaign's report says so ([What a campaign assumed](#what-a-campaign-assumed)).
How the pairs are made -- which sensor output is compared, by what
measure -- is the backend's to say.

### Effects a scenario sets

Some settings are effects with no attribute of the ODD to set: what a
scenario stages, such as the gap to a vehicle about to cut in or how far a
crossing pedestrian is seen. They are drawn from the network itself. A
quantity of the rules gives each (a root when nothing in the domain
conditions it: [A part that reads no input](#a-part-that-reads-no-input)),
and a knob with that `quantity` and no attribute writes it:

```python
sampler = openodd.OddSampler(
    odd,
    effects=[openodd.OddKnob("scenario.npc_gap_m", quantity="vehicle_ahead_gap", limits=(2, 30))],
    backend=backend,                  # whose rules give vehicle_ahead_gap
)
```

Each case draws every quantity at a quantile of its own, at the values the case drew for the inputs that stand
for the ODD's attributes (`concept:`, converted to the input's `unit`; an
input of an attribute's name in version 2 rules). An input that stands for no
attribute reads its `default`, and without one the sampler refuses. A quantity
a quantity reads is drawn at its own quantile too. `limits` and `integer`
apply to the value written, and the case keeps it (`OddSample.quantities`,
recorded as `case.quantities`) for the [case plot](case_plot.md).

Each effect's own quantile is a parameter of the case, `quantile(q)` in its
`values` (`openodd.sampler.quantile_name`), drawn in `[0, 1]` with it. A
[flaky search](flaky.md#the-parameters-normalized) moves it to where the
outcome flips. A quantity an effect reads is drawn at a quantile of its own,
from the seed, the case and the quantity.

A scenario's control (`controls`, keyed by measure) with a `quantity` that no
attribute of the ODD reads is such an effect too. The ODD stays discrete, the
conditions a system is designed for. The effects are continuous, drawn from
distributions, and are what a search for instability moves.

## Chains

A part may read another quantity of the same rules, so a quantity can derive
from one derived itself -- the speed drivers keep from how far they see, and
that from the fog. The rules are then a graph: the inputs and the quantities
are its variables, each part a factor between the variables it reads and its
quantity (the editor draws it so, [editor.md](editor.md#the-factor-graph)).

```yaml
quantities:
  visibility:
    parts:
      fog: {at: {fog_density: [0, 100]}, uniform: {low: [250, 15], high: [400, 40]}}
  sight_speed:
    parts:
      sight:
        at: {visibility: [15, 50, 100, 250, 400]}      # a quantity
        triangular:
          low:  [4.6, 10.0, 15.2, 25.7, 33.4]
          mode: [6.5, 14.1, 21.6, 36.5, 47.3]
          high: [7.6, 16.6, 25.4, 42.9, 55.6]
```

- **A quantity read is worked out first**, at the inputs it reads, and at its
  **own quantile**: `quantiles={"visibility": 0.1}`, the median when not
  named. Each quantity has a quantile of its own, so a run that draws one per
  quantity samples the whole graph, each given what it reads
  (`Rules.sample(inputs, quantiles)`, `Backend.derived_sample`).
- **A quantity given is observed**: a value in `inputs` under a quantity's
  name is taken as it is, and what it reads is not worked out.
  `Rules.quantile("sight_speed", {"visibility": 80}, 0.5)` is the
  distribution given a visibility of 80 m.
- **A cycle is refused**: a quantity may not read itself, through any chain
  (`the quantities read each other in a cycle: a -> b -> a`), and an input the
  file describes may not have a quantity's name.
- `Rules.order` lists the quantities each after those it reads,
  `Rules.upstream(q)` the quantities `q` reads, and `Rules.free_inputs` the
  inputs a caller gives. `Backend(rule_inputs=...)` lists the backend's
  inputs only: a quantity of the rules is always readable.

`examples/wasm-viewer/chain_rules.yaml` chains the weather's grip and
visibility to the speed and the time gap drivers keep.

## Fitting to data

Rules written by hand are a guess. `Rules.fit` estimates them from data
measured in the real world -- a log of drives, each row the values seen
together -- by **maximum likelihood**: the parameters under which the rows
are most likely.

```python
import openodd

rules = openodd.Rules.read("weather.yaml")
rows = openodd.read_rows("drives.csv")       # water, fog, grip, visibility, ...
fit = rules.fit(rows)
report = fit.quantities["grip"]               # a QuantityFit
report.rows, report.skipped                   # the rows it was fitted from, and those missing something it reads
report.log_likelihood_before, report.log_likelihood
fit.rules.range("grip", {"water": 70})       # the fitted rules
fit.write("weather_fitted.yaml", extends="weather.yaml")
backend.use_rules("weather_fitted.yaml")     # runs follow the fitted rules
```

or from the shell:

```sh
openodd-rules-fit weather.yaml drives/*.csv -o weather_fitted.yaml [-q grip] [--weight w]
```

**What a fitted part rests on.** Each part a fit moves gets a `basis` of
kind `fitted` ([The basis of a part](#the-basis-of-a-part)): the rows it
was fitted from, the files they were read from with their SHA-256
(`data_files=`; `openodd-rules-fit` names the files it reads), the
calibration after the fit, who fitted (`by=`, `openodd <version>` by
default) and when (`date=`, today by default). A part the fit holds whole
keeps the basis it has.

**What is estimated.** The rule file's own numbers: every parameter at every
point a part writes. The families, the inputs and their points stay as
written, so the fitted file is the same rules with other numbers, and a
point no row falls near (no row interpolates from it) keeps its value.

| Family | Estimated | Held |
|---|---|---|
| `uniform` | `low`, `high` | |
| `triangular` | `low`, `mode`, `high` | |
| `normal` | `mean`, `sd` | `low`, `high`: the physical range it is truncated to |

**The graph factors.** The rules are a factor graph, each quantity a
conditional distribution given what its parts read, so the likelihood of a
row is the product of each quantity's density at its value given what it
reads there, and the log-likelihood is a sum, one term per quantity. Each
quantity is fitted on its own, from the rows that give it **and** everything
it reads: a row missing the visibility tells nothing of `sight_speed`'s
factor (it is counted in `skipped`), nor of the visibility's. An input
missing from a row reads its `default`, as everywhere in the rules; a
quantity read has none. With several parts, the parts move together, so the
quantity's density at a value is the density of the part deciding it there
(the one whose distribution function is greatest under `min`); a part that
never decides is free, within the bound it puts on the quantity.

**Rows.** A row is a mapping of names to values: a quantity's name gives
its observed value, an input's name the input. Values may be numbers or
numeric strings (a CSV's); `None`, `""` and NaN are not observed, and
columns the rules do not read -- a timestamp -- are ignored. `weights`, or
the column `weight_column` (`--weight COLUMN`) names, weighs each row, as a
time step's duration would; a row without a weight weighs 1. Python, the
command and the editor read rows with the same reader, the core's.
`read_rows` reads CSV and TSV with a header, JSON lines, and JSON: a list of
objects, `{"rows": [...], "weight": "<column>"}` (the weight column, 1 where
a row has none; `openodd-rules-fit` uses it unless `--weight` is given), or
an object of columns. The editor's graph fits the same JSON in the browser
([editor.md](editor.md#fitting-the-graph-to-data)).

**How.** The likelihood is maximized by L-BFGS from the rules' current
values. A row must lie inside a uniform's or a triangular's range, so the
start is widened where one does not, and the fit closes the range on the
data through a log barrier whose weight falls to 0 stage by stage. A
triangular's `mode` is fitted as its share of the range, a normal's `sd` on
a log scale, so each stays valid. A row outside a normal's `low`..`high` is
an error: the range is the file's to say.

**Degenerate data.** A likelihood grows without bound as a distribution
narrows onto one value (every row the same grip, or one row near a point).
`min_spread` (default 0.001) keeps the width at each point with data --
`high - low`, or a normal's spread as truncated (its interquartile range
over 1.349, `sd` when the range is wide) -- at least that share of the
standard deviation of the quantity's values. The truncated spread, not
`sd`, because rows piled on one end of a normal's range let its mean run
far outside it, squeezing the distribution into a spike at that end with
`sd` as it was. Write points where there is data to estimate them
from: the maximum-likelihood estimate of a uniform's range is the data's
own envelope, which is as wide as the data happened to be.

| `Rules` member | |
|---|---|
| `fit(rows, weights=None, *, weight_column=None, quantities=None, max_iterations=1000, tolerance=1e-10, min_spread=1e-3, fixed=())` | A `RulesFit`. `quantities` limits the fit; one named that no row gives is an error. `fixed` holds values besides the file's ([below](#holding-values)) |
| `log_likelihood(rows, weights=None)` | Per quantity, `(log-likelihood, rows)`: how well rules -- fitted or written -- explain data; compare two on the same rows |

| `RulesFit` member | |
|---|---|
| `rules` | The fitted rules |
| `quantities` | Per quantity fitted: a `QuantityFit` -- `rows`, `skipped`, `log_likelihood_before` (`-inf` when a row lay outside the rules' range), `log_likelihood`, `iterations`, `converged`, and per part `points`, `points_with_data` and `estimated` (the values fitted) |
| `text(extends=None)`, `write(path, extends=None)` | The fitted parts as a rule file. With `extends`, the rules fitted (a path, written relative to `path`, or a URL), it extends them and reads as the whole fitted set; without, it merges over them (`Rules.read_over`, `Backend.use_rules`) |

### How well the rules match

A fit's log-likelihood says which of two rule sets is likelier on the same
rows, not whether either matches them. `Rules.calibration(rows)` (and each
`QuantityFit`'s `calibration_before` and `calibration`) says that, from where
each row's value falls in the quantity's distribution at the row's inputs --
its quantile `u = cdf(value)`, the probability integral transform. Under
rules that match the data, `u` is spread evenly over 0..1, whatever the
family:

| `Calibration` | |
|---|---|
| `in_band` | The share of rows in the distribution's central 90%: 0.9 when it matches; less when too narrow or off-centre, more when too wide |
| `histogram` | The share of rows in each tenth of the quantiles: flat when it matches; a U when too narrow, a hump when too wide, a slope when off-centre |
| `ks` | Kolmogorov-Smirnov's distance of `u` from even (about `1.36 / sqrt(rows)` is what chance alone gives 5% of the time) |
| `outside` | The share of rows the rules hold impossible |
| `mean_log_likelihood` | The log-likelihood per unit of weight: comparable across data sets of different sizes |

`openodd-rules-fit` prints the band before and after; the editor shows it
per factor as the data loads, and before and after a fit.

A uniform's or triangular's fitted range closes on the data: its rows at
the ends sit on it. The fitted file writes `low` and `high` rounded outwards
to ten significant digits, so a reader that rounds (the editor writes ten)
still holds every row.

### Holding values

A fit need not move everything. What is known -- a grip measured on a test
track, a spread from the tyre's datasheet, a dry road nobody drove -- is
held as written, and the rest is fitted given it.

The rule file says what its author holds, on the part:

```yaml
quantities:
  tire_friction:
    parts:
      water:
        at: {water: [0, 50, 100]}
        normal: {mean: [0.9, 0.7, 0.5], sd: 0.05, low: 0, high: 1.2}
        fixed: [sd, "mean@water=0"]   # the spread, and the dry road's mean
      ice:
        at: {temperature: [-10, 0, 5]}
        uniform: {low: [0.05, 0.1, 0.8], high: [0.15, 0.3, 1.0]}
        fixed: true                   # all of it
```

| `fixed` entry | Holds |
|---|---|
| `true` | every value of the part |
| `sd` | the parameter at every point |
| `mean@water=0` | the parameter at one point: every input the part reads, each at one of its points (`mean@water=0,rain=50`) |
| `"@water=0"` | every parameter at that point (quote it: YAML reads a bare `@` as reserved) |

and a fit may hold more, without editing the file: `rules.fit(rows,
fixed=[...])` or `openodd-rules-fit --fix SPEC` (repeatable) take the same
entries prefixed by the quantity and the part -- `tire_friction`
(a quantity whole: it is not fitted), `tire_friction.water` (a part),
`tire_friction.water.sd`, `tire_friction.water.mean@water=0`,
`tire_friction.water@water=0`. An entry that names nothing -- a parameter
the family has not, a value not one of the points -- is an error.

The fitted file writes only the distributions, so what the rule file holds
stays held when the fitted file merges over it. Held values shape what is
fitted: the free values are the best given the held ones, not the ones a
free fit would find -- with the dry road's mean held too low, the wet road's
rises to make up for it where the two are interpolated. A uniform's or a
triangular's range held narrower than the data is an error (a row there is
impossible). A triangular's `mode` held stays put while its range is fitted
around it, and a range held leaves the `mode` alone to be fitted inside it.

## Checks

A rule file is checked when it is read, and `RulesError` (a `ValueError`) says
where (`weather_rules.yaml: quantities.tire_friction.parts.water.uniform: at
water=50.0: low (0.95) must not be above high (0.85)`):

- every field known, every name a name, every part with exactly one
  distribution and every parameter of it;
- every input's points increasing, every parameter giving one value per point;
- the distribution valid at every point: `low` not above `high`, a `mode`
  between them, a positive `sd`, a normal distribution's `low`..`high` not
  empty. Each parameter is interpolated linearly, so a distribution valid at
  the points is valid between them too.

A part may write at most 1,000,000 points (the product of its inputs'
points).

Which inputs exist is the backend's to say: `Backend(rule_inputs=[...])`
refuses rules reading any other, its own and a run's alike.

## From version 1

Version 1 wrote fuzzy rules: inputs sorted into fuzzy terms, and rules giving
values per term, inferred as zero-order Sugeno. It is no longer read. Version
1's rule files were nearly all tables written as rules -- one input per rule
base, each term a triangle peaking where its neighbours end -- and inferring
those is interpolating linearly between the peaks. Such a file converts point
for point:

| Version 1 | Version 2 |
|---|---|
| `rule_bases:` | `parts:` |
| an input's terms, peaking at 0, 50, 100 | `at: {water: [0, 50, 100]}` |
| `- if: (water IS wet)` / `then: {low: 0.65, high: 0.85}` | `low` and `high` at the term's peak: `low: [..., 0.65, ...]` |
| a shoulder (`trapezoid: [-90, -90, -6, 0]`) | its inner corner as the outermost point: values hold beyond it |
| `combine: min` | `combine: min` (the parts now move together at one quantile) |

A rule base whose rules read several inputs becomes a part over those inputs,
with a value at every combination of their peaks. Rules joined by `OR` or
`NOT` have no direct counterpart: write the values they meant at the points.

## Extending a rule file

A rule file may `extends` others and write only what it changes over them:
one weather model, and a file per vehicle with its own grip.

```yaml
# vehicles/mini.yaml
version: 2
extends: ../base_weather.yaml        # relative to this file
quantities:
  tire_friction:
    parts:
      water:
        uniform:                     # replaced; the points are base_weather.yaml's
          low:  [0.95, 0.60, 0.30]
          high: [1.05, 0.80, 0.50]
```

`extends` takes one reference or a list, merged in order, each over the
last, and the file itself over them all:

| Reference | Read |
|---|---|
| `base.yaml`, `../shared/base.yaml` | relative to the file naming it -- a local file, a URL, or a file in a git checkout -- or an absolute path |
| `https://example.com/rules/base.yaml` | through the host: Python's `openodd.rules.fetch_url` (urllib, 30 s, at most 4 MB), the browser's `fetch` |
| `{url: https://..., sha256: <64 hex digits>}` | the same, refused unless its content has that digest |
| `{git: <repository>, rev: <tag, branch or commit>, path: <file>}` | from a git repository at a revision, cached as an ODD's [git sources](../README.md) are (`$OPENODD_CACHE`) |

Only `https://` URLs are read. A file that extends itself, through any chain,
is refused.

**How files merge.** A quantity, a part and an input the later file names
merge over the earlier file's of that name, field by field: `combine` and
`parts` of a quantity; `at`, the distribution, `text` and `fixed` of a part;
`default` and `text` of an input. A part's `at`, its distribution and its
`fixed` are each replaced whole, and a distribution of another family replaces the one there (`normal:`
over `uniform:`). `null` removes a quantity, a part or an input. So
`uniform:` alone keeps the points, and `at:` alone keeps the distribution
(which must then have as many values as the new points).

**Inside one file**, YAML anchors share a part (`&points` ... `*points`), and
merge keys take a mapping and change some of it:

```yaml
parts:
  water:
    <<: *water_points     # an anchored {at: ...}
    uniform: {low: [0.9, 0.65, 0.45], high: [1.1, 0.85, 0.65]}
```

A merged rule set is checked as a whole, once merged, so an extending file
need not be one on its own. `Rules.files` lists every file read, the
extended ones first, with the SHA-256 of each: what a run records of the
rules it followed.

## A scenario's nodes

A backend's rules are the network every scenario shares: the weather, and
what follows from it (the road's grip, how far drivers see). A scenario adds
nodes of its own, in a rule file kept with it: what it sets up, such as the
gap to a vehicle about to cut in. An added node may read the shared nodes,
causes and effects alike (a gap that lengthens on a wet road reads the grip).
It may not replace one:

```yaml
# cut_in/rules.yaml
version: 2
basis: {kind: expert, by: Scenario team}
quantities:
  vehicle_ahead_gap:
    parts:
      grip:
        at: {grip: [0.45, 1.1]}
        uniform: {low: [5, 2], high: [40, 30]}
```

```python
backend.add_rules("cut_in/rules.yaml")   # the shared network, and the scenario's nodes
backend.active_rules.upstream("vehicle_ahead_gap")   # ["grip"]
backend.add_rules(None)                  # the shared network alone
```

`Rules.extended(added)` joins them, and `Rules.read_extending(path)` reads a
scenario's file joined to the shared rules. That file is checked as part of
the joined network, so a version 3 node that reads a shared quantity names no
concept for it. The join refuses a quantity both define, an input both
describe otherwise (its `default`, `concept` or `unit`), and taxonomies of
different content, so one scenario can never change what another's cases
follow. Each side keeps its own `basis`: a file-level basis stays with that
file's quantities, and a campaign's report says who set which node. The
backend also checks the joined network against its `rule_inputs`. Merging a file over the rules (`Rules.merged`,
`Backend.use_rules`: a fit, a run's measured weather) is how the shared
network changes, and the scenario's nodes still add to the result. A
sampler's [effects](#effects-a-scenario-sets) are usually the scenario's
nodes.

## The backend

| Member | |
|---|---|
| `rules` | The backend's own rules (`None`: none) |
| `rule_inputs` | The inputs they may read (any, if empty) |
| `rule_input_knobs` | Input → the key of the knob whose written value it is (`precipitation` → `environment.rain`) |
| `rule_input_text` | Input → a line saying what it is |
| `use_rules(path, URL or Rules or None)` | Follow a file's rules merged over `rules` from now on (it need only name what it changes, as an extending file does); `None` goes back to `rules` alone |
| `active_rules` | The rules followed |
| `rule_values(values)` | What the rules read of the values a caller has: by default those. A backend whose rules read something derived -- the larger of `wetness` and `precipitation_deposits` -- overrides it |
| `derived(quantity, values)` | `low`, `high`, `median` and `mean` of the quantity |
| `derived_range(quantity, values)` | `(low, high)` |
| `derived_at(quantity, values, quantile, quantiles=None)` | The value a run that drew `quantile` sets (the quantities it reads at `quantiles`, [Chains](#chains)) |
| `derived_sample(values, quantiles=None)` | Every quantity's value, each at its own quantile |
| `manifest()` | The backend described for tools that do not run it ([below](#the-backends-manifest)) |

A run that reads its own rules should call `use_rules` at its start every
time, with `None` when it names none, so one run's rules do not reach the
next: the backend is usually the process's default one, shared by every run.

## The backend's manifest

`Backend.manifest()` describes a backend as plain data, to write as YAML or
JSON for tools that cannot run it, such as the editor in the browser:

```python
import yaml
yaml.safe_dump(backend.manifest(), open("carla.manifest.yaml", "w"), sort_keys=False)
```

```yaml
openodd_backend: 1
name: CARLA
probes:                     # what a binding may name, with the unit it reads in
  rain: {unit: "", text: "The rain level, from CARLA's 0-100 precipitation."}
readers: [measure]
knobs:                      # by the probe whose attribute it sets
  rain:
    key: environment.rain
    values: {none: 0, light: [1, 30], moderate: [30, 70], heavy: [70, 100]}
rule_inputs:                # what the rules may read, and the knob that writes it
  precipitation: {knob: environment.rain}
  water: {text: "The larger of wetness and precipitation_deposits"}
rules:
  - {file: weather_rules.yaml, sha256: 3195f993...}
```

The probes, readers and knobs are the backend's own. An input's `knob` comes
from `rule_input_knobs`: with it, an ODD attribute can be followed to the
rules. The binding names the attribute's probe, the manifest names the knob
of that probe and the values it writes for each bucket, and it names the rule
inputs the knob writes. The editor draws that chain, and the ODD's buckets
over each part's points ([editor.md](editor.md#the-binding-and-the-connections)).

## Seeing them

The rules view renders a rule file in the browser: each part's distribution
over its input (the range, and the median), what each part and the quantity
give at the inputs chosen, and the quantity's distribution there. It is a
component a page embeds (`RulesView` and `mount(..., {view: "rules"})`,
[wasm.md](wasm.md#rules)); `examples/wasm-viewer/rules.html` puts it beside an
editor of the file.

To edit a rule file as forms rather than text, use the editor's **Rules** mode
([editor.md](editor.md#editing-rules)): drag a point's values on the part's
chart, type them in its table, and watch the quantity follow, read by this
same reader.
