# Sampling by shares of the ODD

The uniform strategy spreads a batch over the ODD alike; the coverage strategy aims it at
what is still uncovered. The **weighted** strategy gives chosen parts of the ODD a share
of the batch -- 30% of the cases in heavy rain, 10% at night in a merge -- and draws the
rest as the uniform strategy would.

```python
sampler = openodd.OddSampler(odd, weights={"heavy_rain": 0.3, "rainy_night_merge": 0.1})
cases = sampler.sample(100)   # 30 in heavy_rain, 10 in rainy_night_merge
```

`weights` implies `strategy="weighted"`.

## The parts are modules

A share is given to a **module**, by its name: the cases in the module are those it holds
for. ASAM OpenODD 1.0 has no construct for a distribution or a share, and allows none of
an application's own in an OpenODD document, so the shares are the sampler's and the
parts they go to are written in the standard's own terms: a module of one condition or of
several, in `MODULES`, beside the ODD or in a file of their own that imports it.

```yaml
# regions.yaml
IMPORT: [urban.yaml]
MODULES:
  heavy_rain:                 # one condition
    INCLUDE_AND:
      rain: heavy
  rainy_night_merge:          # three
    INCLUDE_AND:
      rain: heavy
      illumination: night
      road_type: merge
```

Such a module must not change the ODD:

- it is not one of the ODD's roots. Name the roots under `ODD:` (a document with no
  `ODD:` section takes every module that is not a situation as a root, which would make
  `heavy_rain` part of the ODD). A root takes no share: every case is in it;
- it does not refer to a root (the standard forbids referring to a module of kind ODD).
  It may refer to other modules.

## What a share means

Each module holds its share of the batch, whatever else the batch holds: with
`heavy_rain: 0.3` and `night: 0.3`, 30% of the cases are in heavy rain and 30% at
night, those in both counting towards both. A share is a fraction in `[0, 1]`, not a
weight relative to the others: 0 keeps the module out of the batch, 1 puts every case in
it.

The modules split the ODD into cells, one per pattern of membership (in `heavy_rain`, not
in `night`, ...). Of all the distributions over the cells that give every module its
share, the sampler draws from the one closest to the uniform strategy's (iterative
proportional fitting from the cells' masses under the uniform strategy, estimated from a
pool of uniform cases): inside a cell, the cases are as the uniform strategy draws them,
and modules independent under the uniform strategy stay independent.

The batch is rounded to whole cases per cell, counting the cases drawn before: by this
sampler's earlier batches and, in a batch of samplers on one ODD, by the earlier samplers
(`drawn`, `OddDraws`). Ten samplers drawing one case each with `wet: 0.3` draw three
wet cases between them.

## What is refused

- a name that is no module of the ODD, a root, an inactive module, a share outside
  `[0, 1]`, weights without the weighted strategy or the weighted strategy without them
  (when the sampler is made);
- a module whose truth does not follow from what the knobs set -- it tests an attribute
  no knob draws, so the sampler cannot tell whether a case is in it (on the first draw);
- shares that cannot hold together inside the ODD -- `a: 0.7` and `b: 0.6` for modules
  that never overlap, or a share for a module no case inside the ODD is in. The message
  gives the closest the shares come;
- a cell too rare to fill: after `max_tries` draws per case, the sampler says how many of
  which cell it found.

## With the flaky search

`FlakySearch` draws the part of each batch it does not explore with the sampler's own
strategy, so with the weighted strategy that part holds the shares. The explored part
goes where the runs are unstable, wherever that is.
