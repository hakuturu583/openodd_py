"""The type stub of the compiled core (python/openodd/_core.pyi) against the
core itself: what type checkers read must parse and name what is there.

A broken stub breaks every project type-checking against openodd -- 0.19.1
shipped one, a block of it duplicated and `class Units:` lost, and mypy in
autoware_carla_scenario stopped at "Unexpected indent".
"""

from __future__ import annotations

import ast
from collections import Counter
from pathlib import Path

import openodd
from openodd import _core

STUB = Path(openodd.__file__).parent / "_core.pyi"


def _tree() -> ast.Module:
    return ast.parse(STUB.read_text(encoding="utf-8"), filename=str(STUB))


def test_the_stub_parses() -> None:
    _tree()


def test_no_class_or_function_is_stubbed_twice() -> None:
    names = Counter(
        node.name
        for node in _tree().body
        if isinstance(node, (ast.ClassDef, ast.FunctionDef))
    )
    twice = sorted(n for n, k in names.items() if k > 1)
    assert not twice, f"stubbed more than once: {twice}"


def test_what_the_stub_names_the_core_has() -> None:
    missing = []
    for node in _tree().body:
        if isinstance(node, ast.FunctionDef) and node.name != "__getattr__":
            if not hasattr(_core, node.name):
                missing.append(node.name)
        elif isinstance(node, ast.ClassDef):
            cls = getattr(_core, node.name, None)
            if cls is None:
                missing.append(node.name)
                continue
            for item in node.body:
                if isinstance(item, ast.FunctionDef) and not item.name.startswith("__"):
                    if not hasattr(cls, item.name):
                        missing.append(f"{node.name}.{item.name}")
    assert not missing, f"the stub names what the core has not: {missing}"


def test_the_rules_are_stubbed_whole() -> None:
    """Every method of the rule classes the Python wrapper calls is in the
    stub: what `openodd.rules` is type-checked against."""
    for node in _tree().body:
        if isinstance(node, ast.ClassDef) and node.name in ("RuleSet", "RuleFit"):
            stubbed = {i.name for i in node.body if isinstance(i, ast.FunctionDef)}
            public = {n for n in dir(getattr(_core, node.name)) if not n.startswith("_")}
            assert public <= stubbed, f"{node.name}: not stubbed: {sorted(public - stubbed)}"

