"""Fuzzy rules: how a backend derives what a run sets from what it was given.

A backend's knobs set what an ODD names -- the rain, the sun. What those do
to the rest of a run -- the road's grip, how far drivers see -- is the
backend's own model, which it may write as fuzzy rules in YAML (docs/rules.md)::

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

Inference is Sugeno's, zero order, with simpful's operators (``AND`` the
least, ``OR`` the greatest, ``NOT`` the complement), in the Rust core. A
backend holds its rules (:attr:`~.backend.Backend.rules`), and a run may read
another file over them (:meth:`~.backend.Backend.use_rules`).
"""

from __future__ import annotations

import os
from collections.abc import Mapping, Sequence
from pathlib import Path
from typing import Union

from . import _core

__all__ = ["Rules", "RulesError"]


class RulesError(ValueError):
    """A rule file that cannot be read, or inputs its rules say nothing of."""


class Rules:
    """A rule set: quantities, each rule bases whose outputs ``combine`` joins."""

    def __init__(self, core: "_core.RuleSet") -> None:
        self._core = core

    @classmethod
    def from_text(cls, text: str, source: str = "<rules>") -> "Rules":
        """The rules of a rule file's *text*; *source* names it in messages.

        Raises:
            RulesError: When the rules are not well formed.
        """
        try:
            return cls(_core.RuleSet.parse(text, source))
        except ValueError as e:
            raise RulesError(str(e)) from None

    @classmethod
    def read(cls, path: Union[str, "os.PathLike[str]"]) -> "Rules":
        """The rules of the file at *path*.

        Raises:
            RulesError: When it cannot be read or its rules are not well formed.
        """
        path = Path(path)
        try:
            text = path.read_text(encoding="utf-8")
        except OSError as e:
            raise RulesError(f"cannot read rules {path}: {e}") from e
        return cls.from_text(text, str(path))

    def merged(self, over: "Rules") -> "Rules":
        """These rules with *over*'s quantities in place of their own."""
        return Rules(self._core.merged(over._core))

    @property
    def quantities(self) -> list[str]:
        return self._core.quantities

    def inputs(self, quantity: str) -> list[str]:
        """The inputs *quantity*'s rule bases read."""
        return self._call(self._core.inputs, quantity)

    def outputs(self, quantity: str) -> list[str]:
        return self._call(self._core.outputs, quantity)

    def source(self, quantity: str) -> str:
        """The file *quantity*'s rules were read from."""
        return self._call(self._core.source, quantity)

    def evaluate(self, quantity: str, inputs: Mapping[str, float]) -> dict[str, float]:
        """Each output of *quantity* at *inputs*; an input not given reads its
        ``default``, and one outside its universe is clamped to it.

        Raises:
            RulesError: On an unknown quantity, an input not given that has no
                default, or inputs no rule fires at.
        """
        values = {str(k): float(v) for k, v in inputs.items()}
        return self._call(self._core.evaluate, quantity, values)

    def range(self, quantity: str, inputs: Mapping[str, float]) -> tuple[float, float]:
        """``(low, high)`` of a quantity whose outputs are ``low`` and ``high``."""
        out = self.evaluate(quantity, inputs)
        if "low" not in out or "high" not in out:
            raise RulesError(
                f"{self.source(quantity)}: {quantity} gives {sorted(out)}, not low and high"
            )
        return out["low"], out["high"]

    def check_inputs(self, allowed: Sequence[str]) -> None:
        """Refuse a rule base reading an input not in *allowed*."""
        self._call(self._core.check_inputs, [str(a) for a in allowed])

    @staticmethod
    def _call(f, *args):  # type: ignore[no-untyped-def]
        try:
            return f(*args)
        except ValueError as e:
            raise RulesError(str(e)) from None

    def __repr__(self) -> str:
        return f"Rules({', '.join(self.quantities)})"
