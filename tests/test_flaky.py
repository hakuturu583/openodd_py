"""Flaky cases: the GP classifier, its metrics, and the search beside the sampler."""

from __future__ import annotations

import pytest

import openodd
from openodd import (
    FlakinessModel,
    FlakyMetrics,
    FlakySearch,
    OddKnob,
    OddSampler,
    ParameterSpace,
)

from test_sampling import CONTROLS, backend, odd

#: Where the system under test stops passing: a gap of 20 m.
BOUNDARY = 20.0


def passes(case) -> bool:
    return case.values["gap"] > BOUNDARY


def search(**kw) -> FlakySearch:
    sampler = OddSampler(odd(), controls=CONTROLS, seed=3, backend=backend())
    return FlakySearch(sampler, **kw)


def observed(search: FlakySearch, count: int = 40) -> FlakySearch:
    for case in search.sampler.sample(count):
        search.observe(case, passes(case))
    return search


def test_the_space_is_what_the_knobs_can_set() -> None:
    space = search().space
    assert space.numeric == {"gap": (2.0, 35.0)}
    assert space.categorical == {"rain": ["none", "light", "heavy"]}
    # In the ODD's order: rain one-hot, then gap scaled over 2..35 m.
    assert space.encode({"gap": 18.5, "rain": "light"}) == pytest.approx([0.0, 2**-0.5, 0.0, 0.5])


def test_with_no_runs_the_sampler_draws_them_all() -> None:
    cases = search().sample(6)
    assert len(cases) == 6
    assert all(c.flaky is None for c in cases)


def test_a_batch_keeps_the_sampler_and_explores_the_boundary() -> None:
    s = observed(search(explore_fraction=0.5))
    cases = s.sample(10)
    assert [c.index for c in cases] == list(range(10))
    own, explored = cases[:5], cases[5:]
    assert all(c.flaky is None for c in own)
    assert all(isinstance(c.flaky, FlakyMetrics) for c in explored)
    # Only what the ODD admits, as the sampler draws it.
    for c in cases:
        assert not (c.buckets["rain"] == "heavy" and c.values["gap"] < 10)
        assert 2 <= c.values["gap"] <= 35

    def spread(batch):
        return sum(abs(c.values["gap"] - BOUNDARY) for c in batch) / len(batch)

    assert spread(explored) < spread(own)
    assert spread(explored) < 6.0
    assert min(c.flaky.flip_probability for c in explored) > 0.05


def test_the_metrics_tell_the_boundary_from_the_far_side() -> None:
    s = observed(search())
    near = s.metrics({"gap": 20.5, "rain": "none"})
    far = s.metrics({"gap": 34.0, "rain": "none"})
    assert near.flip_probability > far.flip_probability
    assert near.instability_confidence >= far.instability_confidence
    assert near.robustness_radius is not None and far.robustness_radius is not None
    assert near.robustness_radius < far.robustness_radius
    # 34 m is 14 m, about 0.42 of the 33 m range, from the boundary.
    assert far.robustness_radius == pytest.approx(14 / 33, abs=0.1)
    assert far.pass_probability > 0.8


def test_the_ranking_puts_the_runs_at_the_boundary_first() -> None:
    s = observed(search())
    ranking = s.ranking()
    assert len(ranking) == 40
    values, passed, metrics = ranking[0]
    assert abs(values["gap"] - BOUNDARY) < 5
    assert metrics.flip_probability >= ranking[-1][2].flip_probability


def test_the_same_seed_and_runs_give_the_same_batch() -> None:
    a = [c.values for c in observed(search()).sample(8)]
    b = [c.values for c in observed(search()).sample(8)]
    assert a == b


def test_a_space_of_any_parameters() -> None:
    space = ParameterSpace(numeric={"speed_kph": (0, 120), "friction": (0.2, 1.0)}, categorical={"lane": [1, 2]})
    model = FlakinessModel(space, seed=1)
    for speed in range(0, 121, 10):
        for friction in (0.2, 0.6, 1.0):
            # Brakes in time while slow or on a grippy road.
            model.observe({"speed_kph": speed, "friction": friction, "lane": 1}, speed < 40 + 60 * friction)
    edge = model.metrics({"speed_kph": 76, "friction": 0.6, "lane": 1})
    safe = model.metrics({"speed_kph": 10, "friction": 1.0, "lane": 1})
    assert edge.flip_probability > safe.flip_probability
    assert safe.pass_probability > 0.8
    scales = model.kernel()["length_scales"]
    assert set(scales) == {"speed_kph", "friction", "lane"}
    assert all(v > 0 for v in scales.values())
    # A run at the same setting both passing and failing is flaky where it stands.
    toss = FlakinessModel(space, [({"speed_kph": 50, "friction": 0.5, "lane": 2}, i % 2 == 0) for i in range(12)])
    m = toss.metrics({"speed_kph": 50, "friction": 0.5, "lane": 2})
    assert m.instability_confidence > 0.9 and m.flip_probability > 0.3


def test_what_cannot_be_measured_is_refused() -> None:
    with pytest.raises(ValueError, match="explore_fraction"):
        search(explore_fraction=1.5)
    with pytest.raises(ValueError, match="no observations"):
        search().metrics({"gap": 10.0, "rain": "none"})
    s = observed(search(), 5)
    with pytest.raises(ValueError, match="none of"):
        s.metrics({"gap": 10.0, "rain": "snow"})
    with pytest.raises(ValueError, match="threshold"):
        FlakinessModel(ParameterSpace(numeric={"x": (0, 1)}), threshold=2.0)
    with pytest.raises(ValueError, match="finite"):
        ParameterSpace(numeric={"x": (1, 0)})


def test_it_is_exported() -> None:
    assert openodd.FlakySearch is FlakySearch
    assert OddKnob  # the sampler's knobs set the space
