# Fuzzy rules

A backend's knobs set what an ODD names: the rain, the sun. What those do to
the rest of a run -- the road's grip, how far drivers see -- is the backend's
own model. It may write it as fuzzy rules: a YAML file that `openodd.Rules`
reads, which the Rust core infers.

```python
import openodd

backend = openodd.Backend(
    rules=openodd.Rules.read("grip.yaml"),   # the file below
    rule_inputs=["water"],
)
backend.derived_range("tire_friction", {"water": 70.0})   # (0.57, 0.77)
backend.use_rules("measured_grip.yaml")   # a run's rules, over the backend's
backend.use_rules(None)                   # back to the backend's alone
```

## The rule file

```yaml
version: 1
quantities:
  tire_friction:
    rule_bases:
      water:
        inputs:
          water:
            universe: [0, 100]
            terms:
              dry: {trapezoid: [0, 0, 0, 50]}
              wet: {triangle: [0, 50, 100]}
              flooded: {trapezoid: [50, 100, 100, 100]}
        rules:
          - if: (water IS dry)
            then: {low: 0.90, high: 1.10}
          - if: (water IS wet)
            then: {low: 0.65, high: 0.85}
          - if: (water IS flooded)
            then: {low: 0.45, high: 0.65}
```

| Key | Meaning |
|---|---|
| `version` | `1` |
| `quantities` | Name → a quantity: `rule_bases`, and `combine` |
| `combine` | How the rule bases' outputs are joined, output by output: `min` (the default) or `max` |
| `rule_bases` | Name → a rule base: `inputs` and `rules` |
| `inputs` | Name → `universe: [low, high]`, `terms`, and an optional `default` |
| `terms` | Name → `triangle: [a, b, c]`, `trapezoid: [a, b, c, d]` or `gaussian: [mean, sigma]` |
| `rules` | A list of `if:` (an antecedent) and `then:` (output → value) |

- **Terms.** A triangle rises from `a`, is 1 at `b` and falls to `c`; a
  trapezoid is 1 from `b` to `c`. An edge whose two points coincide is a
  shoulder: 1 beyond it (`trapezoid: [0, 0, 0, 50]` is 1 at and below 0).
- **Antecedents.** Clauses `(input IS term)`, joined by `AND` (the least of
  them) or `OR` (the greatest), and `NOT` (one less it). One group takes one of
  `AND` and `OR`: parenthesize to mix them, `((a IS x) OR (b IS y)) AND (c IS z)`.
- **Inference** is Sugeno's, zero order: an output is the rules' values of it,
  averaged by how strongly each rule fires. Triangles that peak where their
  neighbours end split every value between two terms, so rules naming one term
  each read linearly between the peaks: a table, written as rules.
- **Inputs** outside their universe are clamped to it. One not given reads its
  `default`, and without one is an error.
- Every rule of a rule base gives the same outputs, and so does every rule
  base of a quantity. A rule giving `low` and `high` may not give a `low` above
  its `high`, and `Rules.range()` reads a quantity with those two outputs.

The operators are [simpful](https://github.com/aresio/simpful)'s (`AND` min,
`OR` max, `NOT` the complement, zero-order Sugeno), so a rule file means the
same in either: `tests/rules_golden.json` holds simpful's outputs, written by
`tools/rules_golden.py`, that the tests hold the Rust rules to. The rules read
a little more than simpful parses: a `NOT` clause may come first in an `AND` or
an `OR`.

## Checks

A rule file is checked when it is read, and `RulesError` (a `ValueError`) says
where (`weather_rules.yaml: quantities.tire_friction.rule_bases.water.rules[1].if: ...`):

- every field known, every term and rule well formed, every clause naming an
  input of its rule base and a term of that input;
- every input's universe covered by its terms;
- some rule firing everywhere the inputs can be, so no weather the inputs can
  hold is left without a value. The terms are piecewise linear between their
  corners, so a gap -- in what the terms cover, or in where the rules fire --
  begins and ends at a corner: the checks read each input at its terms'
  corners (and a bell's centre), a few evenly spaced points and the midpoint
  of each two neighbours, and a rule base at every combination of those. One
  whose combinations number more than a million is refused, to be split into
  rule bases reading fewer inputs.

Which inputs exist is the backend's to say: `Backend(rule_inputs=[...])`
refuses rules reading any other, its own and a run's alike.

## The backend

| Member | |
|---|---|
| `rules` | The backend's own rules (`None`: none) |
| `rule_inputs` | The inputs they may read (any, if empty) |
| `use_rules(path or Rules or None)` | Follow a file's rules over `rules` from now on, quantity by quantity; `None` goes back to `rules` alone |
| `active_rules` | The rules followed |
| `rule_values(values)` | What the rules read of the values a caller has: by default those. A backend whose rules read something derived -- the larger of `wetness` and `precipitation_deposits` -- overrides it |
| `derived(quantity, values)` | Each output of the quantity |
| `derived_range(quantity, values)` | `(low, high)` of a quantity giving those two |

A run that reads its own rules should call `use_rules` at its start every
time, with `None` when it names none, so one run's rules do not reach the
next: the backend is usually the process's default one, shared by every run.

## Seeing them

The rules view renders a rule file in the browser: each input's terms, which
rules fire at the inputs chosen, what each rule base and the quantity give,
and the quantity's response as one input moves. It is a component a page
embeds (`RulesView` and `mount(..., {view: "rules"})`,
[wasm.md](wasm.md#fuzzy-rules)); `examples/wasm-viewer/rules.html` puts it
beside an editor of the file.
