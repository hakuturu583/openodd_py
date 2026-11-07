# Drawn and measured: how well a scenario stages its cases

A case's `values` are what the sampler **drew**: the ODD values it aimed at.
They reach a run through the scenario's knobs. A control usually writes
`scale × value + offset` into a scenario parameter. For example,
`dynamic.vehicle_ahead_gap` is written to `scenario.npc_ahead_m` with
offset 11. The run's probes then **measure** what the scenario actually
staged, step by step, in the coverage document's `odd.samples` (`t`, `lat`,
`lon`, then one column per ODD attribute).

When a knob is miscalibrated, the two differ systematically:

- a wrong **offset** is a constant bias;
- a wrong **scale** is an error that grows with the value.

The case plot and its background then show the runs at values they never
had. The views below make the difference visible and say how to fix the
knob.

## The measured value, and its window

An attribute's **measured value** is its first sample, from t = 0, within
the **measurement window** (`measured_at`, 1 s by default, at most 10 s)
that has a value. The scenario has staged its start, and the dynamics have
not yet moved it far.

A case run several times gets the mean of its runs' measured numbers, or
the most common measured label. Attributes that change quickly after the
start, such as a closing gap, are measured one tick in. Keep the window
short, so that the dynamics do not show up as drift.

## Effects: what a scenario sets up

What a scenario sets up -- the gap to a vehicle about to cut in, how fast a
pedestrian crosses -- is no ODD attribute but an **effect**: a node of the
rules the sampler draws with each case (rules.md, "Effects a scenario sets").
The value drawn is the case's `quantities`. A run measures it with a probe
per quantity, beside the ODD's attributes and not as one -- no cover item, no
grade -- sampled on every tick of its first `EFFECT_WINDOW` (10) seconds:

```python
collector = openodd.CoverageCollector(
    items, odd=odd, backend=backend,
    effects={"vehicle_ahead_gap": gap_probe},   # quantity -> its probe
    effects_table="separate",                   # or "odd"
)
```

Where they are written is for the analysis to choose:

| `effects_table` | Written as |
|---|---|
| `separate` (the default) | a table of their own, `effects.samples`: `quantities`, and rows `[t, value, ...]` |
| `odd` | columns of the ODD's table, `odd.samples`, named `quantity:<name>` -- one table for a reader that wants it so |

`openodd.move_effect_samples(document, "odd" or "separate")` moves them from
one to the other in a file already written. The views read both: each effect
measured is placed, and drifts, like an attribute -- drawn from
`case.quantities`, measured from its column -- and its correction is the
knob that wrote it (`scale`, `offset`). `drift_rules` moves the points of a
part reading a drawn attribute's input, so it does not correct an effect:
a root of the network has no such points. An effect no probe measured is
left out of the drift view:
a CARLA weather field the rules give, written as it is drawn, has nothing to
drift.

## The case plot on either basis

`CasePlot.html(basis="measured")` places the cases by their measured
values. `basis="drawn"` is the default. A **Drawn / Measured** toggle next
to the axis pickers switches between them:

- in the explorer (`page()`, `--cases-html`), both bases are rendered ahead
  of time;
- in a host-driven fragment, the toggle dispatches `openodd:axes` with
  `detail.basis`;
- in the WASM viewer, it re-renders live.

On the measured basis, the flaky model behind the background is fitted on
the measured coordinates. Its card shows both values ("12.3 · measured
14.5"), and the axes span both bases, so they do not move when the basis
does.

`drift_leaders=True` draws a short line from each mark to its value on the
other basis.

## Scenario realisation (the drift view)

```python
plot = openodd.CasePlot(documents)
plot.drift(knobs={"dynamic.vehicle_ahead_gap": (1.0, 11.0)})  # as data
plot.drift_html(knobs={"dynamic.vehicle_ahead_gap": (1.0, 11.0)})
```

```bash
openodd-coverage results/ --drift-html drift.html --knob dynamic.vehicle_ahead_gap=1,11
```

**A summary table, worst first.** One row per attribute, sorted by the
lowest bucket hit rate first, then the largest error relative to the
attribute's range. Each row holds:

- the runs;
- the **bias**: the mean of measured − drawn;
- the **MAE**;
- the largest |error|;
- the **bucket hit rate**: the share of runs whose measured value fell in the
  same ODD bucket as the drawn one;
- the fit.

**A numeric attribute** gets a scatter with one point per run:

- drawn on x, measured on y;
- the identity (`measured = drawn`), dashed, and the fitted line;
- the ODD bucket edges on both axes;
- the same pass and fail marks as the case plot;
- a ring around each run that landed in another bucket than it was drawn
  in.

Hovering a point shows its case, run, both values, both buckets and the
error.

**A categorical attribute** gets an agreement table: how often each drawn
label was measured as each label.

### Reading it to recalibrate a knob

For each numeric attribute, `measured ≈ a × drawn + b` is fitted by least
squares over the runs. The fit reports n, a, b, the residual's standard
deviation σ, and R².

- **`a ≈ 1`, `b ≠ 0`:** the knob's offset is off by `b`.
- **`a ≠ 1`:** its scale is off by the factor `a`.

The view suggests the correction when the fit can be trusted:

> Write the drawn value d as (d − b) / a through the existing knob.

For a knob that writes `scale × value + offset`, that is the same knob with:

```
new_scale  = scale / a
new_offset = offset − scale × b / a
```

The coverage document does not hold the knob's scale and offset. Pass them
as `knobs={attribute: (scale, offset)}`, or `--knob ATTRIBUTE=SCALE,OFFSET`,
to get the new values.

The fit is **not** trusted, and the view says why, when:

| Condition | Why it is not trusted |
|---|---|
| fewer than 5 runs (`drift::MIN_FIT_RUNS`) | too few to fit |
| `\|a\|` below 0.1 (`MIN_SLOPE`) | the measured value does not follow the drawn one |
| σ above a quarter of the attribute's median bucket width (`MAX_RESIDUAL`; 2% of the range without buckets) | too noisy for a constant correction |
| another drawn attribute explains more than half of the residual's variance (`MAX_STRUCTURE`: r² for a number, adjusted η² for a label) | the error depends on that attribute, e.g. the rain |

In the last two cases, no constant offset and scale can fix the knob.

**A knob that takes its value from the rules** (`OddKnob(quantity=...)`,
[rules.md](rules.md#knobs-from-the-rules)) is corrected in the rules, not in
its scale and offset: for a drawn `x` its quantity should now give what it
gives at `(x − b) / a`, so each part reading the attribute's input has its
points moved from `p` to `a·p + b`, its distributions as they are.
`CasePlot.drift_rules(rules, {attribute: quantity})` writes that as a rule
file over the rules, each moved part with a `basis` of kind `fitted` (the
runs, R², σ); `openodd-coverage --drift-rules FILE --rules RULES
--knob-quantity ATTRIBUTE=QUANTITY` writes it to `FILE`, extending `RULES`.
Use it with `Backend.use_rules(FILE)`, or merge it in. Nothing is written
when the fit suggests nothing, and a fit with a negative slope is refused.

**Calibrated.** A fit that stays within `max(2σ, 5% of the bucket width)`
of `measured = drawn` over the drawn range needs no correction
(`NEGLIGIBLE`).

### What sweep 2 says

On the 49 runs of the VisionPilot cut-in sweep (`odd-sweep2`):

- **`dynamic.vehicle_ahead_gap`** is staged **2.25 m** long on average
  (MAE 2.25 m, max 2.35 m; every run shifted). Its fit is
  `measured ≈ 1.002 × drawn + 2.22 m` (R² 1.000, σ 0.08 m). **33%** of the
  runs landed in another gap bucket than they were drawn in. The view
  suggests writing `d − 2.22 m` through the knob: with its offset 11, offset
  ≈ 8.8.
- **`dynamic.vehicle_ahead_relative_speed`** is within 0.21 km/h: calibrated.
- **Illumination, rain and fog** are staged as drawn: 100% agreement.

## Data

`CasePlot.drift()` returns, per attribute:

- `summary`: `runs`, `bias`, `mae`, `max_error`, `bucket_hit_rate`,
  `shifted`;
- for a numeric attribute:
  - `runs`: each with `drawn`, `measured`, `error`, both buckets and the
    outcome;
  - `buckets`;
  - `fit`: `a`, `b`, `residual_std`, `r2`;
  - `calibration`: `status`, plus `write_as` (`scale` 1/a, `offset` −b/a)
    when it suggests one, or the reason when it does not; with
    `drift(knobs={attribute: (scale, offset)})`, a suggestion also has
    `knob`: the current `scale` and `offset` and the `corrected` ones;
- for a categorical attribute: `counts` (drawn × measured).

`CasePlot.data(basis=...)` gives each case's `values` on the basis and its
`other_values` on the other basis.

In Rust, these are `CaseSet::drift`, `drift::{Fit, Calibration,
corrected_knob}` and `case_plot::drift`. In the browser, they are
`Viewer.drift_html` / `drift_json`, and `mount(viewer, el, {view: "drift"})`.
