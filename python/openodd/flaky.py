"""Find flaky cases: where a run's pass/fail outcome flips under small changes.

The ODD's coverage spreads runs over its buckets; instability sits on the
boundary between passing and failing, which coverage alone does not look for.
This module learns that boundary from the runs so far -- Gaussian-process
classification of pass/fail, in the Rust core -- and says how flaky a case is:

* :attr:`FlakyMetrics.flip_probability` -- the estimated chance that a small
  change of the case (within ``epsilon`` in the normalized space) flips its
  outcome; a case whose outcome is a coin toss counts too;
* :attr:`FlakyMetrics.instability_confidence` -- the posterior probability that
  that flip rate exceeds ``threshold``;
* :attr:`FlakyMetrics.robustness_radius` -- the smallest normalized distance at
  which the predicted outcome changes (``None``: nowhere in the space).

Parameters are normalized (:class:`ParameterSpace`): a number scaled to
``[0, 1]`` over its range, a category one-hot so that two are 1 apart, so
parameters of any unit or kind -- an ODD's attributes, a scenario's own -- are
measured together.

:class:`FlakySearch` joins it to an :class:`~.sampler.OddSampler`: part of each
batch is drawn by the sampler's own policy (uniform, or the least covered
first), the rest are the most unstable of the candidates the ODD admits::

    search = FlakySearch(sampler, explore_fraction=0.5)
    for case, passed in earlier_runs:
        search.observe(case, passed)
    for case in search.sample(20):
        run(case.overrides)                 # case.flaky: why it was chosen
    search.ranking()                        # the runs so far, flakiest first

The model: a latent ``f ~ GP(0, k)`` with a squared-exponential kernel and
``P(pass | f) = Φ(f)``, its posterior the Laplace approximation (Rasmussen &
Williams, algorithms 3.1 and 3.2). The kernel has a length scale per parameter
(a parameter the outcome does not depend on gets a long one): they start from
a GP regression on the outcomes fitted by the ``friedrich`` crate and are
refined, with the amplitude, by the Laplace evidence.
"""

from __future__ import annotations

import logging
from collections.abc import Iterable, Mapping, Sequence
from dataclasses import dataclass
from typing import Any, Optional, Union

from . import _core
from .model import to_core
from .sampler import OddSample, OddSampler

logger = logging.getLogger(__name__)

__all__ = ["FlakinessModel", "FlakyMetrics", "FlakySearch", "ParameterSpace"]

Case = Union[OddSample, Mapping[str, Any]]
Observation = tuple[Case, bool]


@dataclass(frozen=True)
class FlakyMetrics:
    """How flaky a case is (see the module docstring).

    Attributes:
        pass_probability: P(pass) at the case.
        flip_probability: The estimated chance a small change flips its outcome.
        instability_confidence: The posterior probability that the flip rate
            exceeds the model's ``threshold``.
        robustness_radius: The smallest normalized distance at which the
            predicted outcome changes; ``None`` when nothing in the space does.
    """

    pass_probability: float
    flip_probability: float
    instability_confidence: float
    robustness_radius: Optional[float]

    @classmethod
    def _from_core(cls, core: Sequence[Any]) -> "FlakyMetrics":
        return cls(*core)


def _values(case: Case) -> dict[str, Any]:
    values = case.values if isinstance(case, OddSample) else case
    return {str(k): to_core(v) for k, v in values.items()}


class ParameterSpace:
    """The parameters of a case, normalized.

    Args:
        numeric: Name -> ``(low, high)``: scaled to ``[0, 1]`` over it.
        categorical: Name -> its labels: one-hot, two labels 1 apart.

    Parameters are coded in the order given, the numeric ones first.
    """

    def __init__(
        self,
        numeric: Optional[Mapping[str, tuple[float, float]]] = None,
        categorical: Optional[Mapping[str, Sequence[Any]]] = None,
    ) -> None:
        from .items import value_label  # noqa: PLC0415 - items imports model

        dims = [("numeric", str(n), (float(lo), float(hi)), None) for n, (lo, hi) in (numeric or {}).items()]
        dims += [
            ("categorical", str(n), None, [value_label(v) for v in labels])
            for n, labels in (categorical or {}).items()
        ]
        self._core = _core.ParamSpace(dims)

    @classmethod
    def _from_core(cls, core: Any) -> "ParameterSpace":
        self = cls.__new__(cls)
        self._core = core
        return self

    @property
    def numeric(self) -> dict[str, tuple[float, float]]:
        return {name: rng for kind, name, rng, _ in self._core.dims if kind == "numeric"}

    @property
    def categorical(self) -> dict[str, list[str]]:
        return {name: labels for kind, name, _, labels in self._core.dims if kind == "categorical"}

    @property
    def names(self) -> list[str]:
        return [name for _, name, _, _ in self._core.dims]

    def encode(self, case: Case) -> list[float]:
        """A case's values, coded in the normalized space."""
        return list(self._core.encode(_values(case)))

    def __repr__(self) -> str:
        return f"ParameterSpace(numeric={self.numeric!r}, categorical={self.categorical!r})"


class FlakinessModel:
    """A pass/fail classifier over a :class:`ParameterSpace`, and its metrics.

    Args:
        space: The parameters a case is coded by.
        observations: Earlier runs: ``(case, passed)``, a case an
            :class:`~.sampler.OddSample` or its values by parameter name.
        threshold: The flip rate above which a case is unstable.
        epsilon: The size of a small change, in the normalized space.
        perturbations: Neighbours drawn within ``epsilon`` per case.
        draws: Joint posterior draws per case.
        directions: Random directions searched for the robustness radius,
            besides the axes.
        seed: The same seed and observations give the same metrics.
    """

    def __init__(
        self,
        space: ParameterSpace,
        observations: Iterable[Observation] = (),
        *,
        threshold: float = 0.1,
        epsilon: float = 0.05,
        perturbations: int = 32,
        draws: int = 256,
        directions: int = 16,
        seed: Union[int, str] = 0,
    ) -> None:
        self.space = space
        self._core = _core.FlakinessModel(
            space._core,
            seed=str(seed),
            threshold=float(threshold),
            epsilon=float(epsilon),
            perturbations=int(perturbations),
            draws=int(draws),
            directions=int(directions),
        )
        self._observed: list[tuple[dict[str, Any], bool]] = []
        for case, passed in observations:
            self.observe(case, passed)

    def __len__(self) -> int:
        return len(self._observed)

    def observe(self, case: Case, passed: bool) -> None:
        """Record one run: its case and whether it passed."""
        values = _values(case)
        self._core.observe(values, bool(passed))
        self._observed.append((values, bool(passed)))

    def metrics(self, case: Case) -> FlakyMetrics:
        """How flaky *case* is.

        Raises:
            ValueError: With no observations, or a case the space cannot code.
        """
        return FlakyMetrics._from_core(self._core.metrics(_values(case)))

    def kernel(self) -> dict[str, Any]:
        """The fitted kernel: ``length_scales`` (by parameter: a long one for a
        parameter the outcome does not depend on), ``amplitude``,
        ``log_evidence``."""
        ls, ampl, evidence = self._core.kernel()
        return {
            "length_scales": dict(zip(self.space.names, ls)),
            "amplitude": ampl,
            "log_evidence": evidence,
        }

    def ranking(self) -> list[tuple[dict[str, Any], bool, FlakyMetrics]]:
        """The runs so far, flakiest first: ``(values, passed, metrics)``."""
        return [
            (*self._observed[i], FlakyMetrics._from_core(m)) for i, m in self._core.ranking()
        ]


class FlakySearch:
    """An :class:`~.sampler.OddSampler` that also looks for flaky cases.

    Args:
        sampler: Draws the cases: its own policy for the share not explored,
            and the candidates for the rest (so only what the ODD admits).
        observations: Earlier runs, ``(case, passed)``.
        explore_fraction: The share of each batch chosen for instability once
            the ODD is covered; the rest is the sampler's own. Nothing is
            explored before a run is observed.
        pool: Candidates drawn per explored case.
        coverage_depth: The holes first: while the sampler's coverage strategy
            has buckets covered fewer than this many times, the cases it needs
            to cover them are its own, whatever ``explore_fraction`` says. 0
            explores from the first observed run; the uniform strategy has no
            holes to fill.
        **model: :class:`FlakinessModel`'s options (``threshold``,
            ``epsilon``, ...).
    """

    def __init__(
        self,
        sampler: OddSampler,
        observations: Iterable[Observation] = (),
        *,
        explore_fraction: float = 0.5,
        pool: int = 16,
        coverage_depth: int = 1,
        **model: Any,
    ) -> None:
        if not 0.0 <= explore_fraction <= 1.0:
            raise ValueError("FlakySearch: explore_fraction must be in [0, 1]")
        self.sampler = sampler
        self.explore_fraction = float(explore_fraction)
        self.pool = int(pool)
        if coverage_depth < 0:
            raise ValueError("FlakySearch: coverage_depth must not be negative")
        self.coverage_depth = int(coverage_depth)
        self.model = FlakinessModel(
            ParameterSpace._from_core(sampler._core.space), observations, **model
        )

    @property
    def space(self) -> ParameterSpace:
        """The drawn attributes, normalized over what their knobs can set."""
        return self.model.space

    def observe(self, case: Case, passed: bool) -> None:
        """Record one run: its case and whether it passed."""
        self.model.observe(case, passed)

    def metrics(self, case: Case) -> FlakyMetrics:
        """How flaky *case* is."""
        return self.model.metrics(case)

    def ranking(self) -> list[tuple[dict[str, Any], bool, FlakyMetrics]]:
        """The runs so far, flakiest first."""
        return self.model.ranking()

    def sample(self, count: int) -> list[OddSample]:
        """Draw *count* cases: the sampler's first, then those chosen for
        instability (their :attr:`~.sampler.OddSample.flaky` set)."""
        if count < 0:
            raise ValueError("FlakySearch: count must not be negative")
        drawn = self.sampler._core.search(
            self.model._core,
            count,
            explore_fraction=self.explore_fraction,
            pool=self.pool,
            coverage_depth=self.coverage_depth,
        )
        for warning in self.sampler._core.take_warnings():
            logger.warning(warning)
        return [
            OddSample._from_core(core, None if m is None else FlakyMetrics._from_core(m))
            for core, m in drawn
        ]
