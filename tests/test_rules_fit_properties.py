"""Properties of fitting rules to data, and of the distributions it fits,
over random rules and rows (Hypothesis).

Each regular run is derandomized: the same examples every time, so CI is
reproducible. `OPENODD_FUZZ_EXAMPLES=<n>` runs `n` random examples per test
instead -- what the weekly fuzz workflow does (.github/workflows/fuzz.yml).
The bugs these found are kept as explicit examples (`@example`) and as unit
tests in src/rules.rs and src/rules_fit.rs.
"""

from __future__ import annotations

import itertools
import json
import math
import os
from pathlib import Path

import pytest
from hypothesis import HealthCheck, example, given, settings
from hypothesis import strategies as st

import openodd
from openodd import Rules, RulesError

FUZZ = os.environ.get("OPENODD_FUZZ_EXAMPLES")
SETTINGS = settings(
    max_examples=int(FUZZ) if FUZZ else 60,
    derandomize=not FUZZ,
    deadline=None,
    suppress_health_check=list(HealthCheck),
    database=None,
)

FAMILIES = ["uniform", "triangular", "normal"]


@st.composite
def dist_params(draw, family: str, scale: float) -> dict[str, float]:
    low = draw(st.floats(-50, 50))
    width = draw(st.floats(0.01, 50))
    if family == "uniform":
        params = {"low": low, "high": low + width}
    elif family == "triangular":
        params = {"low": low, "mode": low + draw(st.floats(0, 1)) * width, "high": low + width}
    else:
        params = {
            "mean": draw(st.floats(low - width, low + 2 * width)),
            "sd": draw(st.floats(0.01, 20)),
            "low": low,
            "high": low + width,
        }
    return {k: v * scale for k, v in params.items()}


def nested(values: list[float], shape: list[int]) -> str:
    if len(shape) <= 1:
        return "[" + ", ".join(repr(v) for v in values) + "]"
    step = len(values) // shape[0]
    return "[" + ", ".join(nested(values[i * step:(i + 1) * step], shape[1:]) for i in range(shape[0])) + "]"


@st.composite
def rule_files(draw) -> tuple[str, list[tuple[str, str, str, dict, str]]]:
    """A rule file of one or two quantities (the second may read the
    first), each one to three parts of any family over one or two inputs,
    some holding values (`fixed:`); all scaled by a power of ten. And the
    holds it writes: (quantity, part, kind, points, first parameter)."""
    scale = 10.0 ** draw(st.integers(-8, 8))
    holds = []
    lines = ["version: 2", "quantities:"]
    for qi in range(draw(st.integers(1, 2))):
        q = f"q{qi}"
        lines += [f"  {q}:", f"    combine: {draw(st.sampled_from(['min', 'max']))}", "    parts:"]
        for pi in range(draw(st.integers(1, 3))):
            family = draw(st.sampled_from(FAMILIES))
            inputs = ["a", "b"] + (["q0"] if qi == 1 else [])
            reads = draw(st.lists(st.sampled_from(inputs), min_size=1, max_size=2, unique=True))
            axes = {}
            for i in reads:
                n = draw(st.integers(1, 4))
                points = draw(st.lists(st.floats(-100, 100), min_size=n, max_size=n))
                axes[i] = sorted({p * scale for p in points})
            shape = [len(v) for v in axes.values()]
            size = math.prod(shape)
            ds = [draw(dist_params(family, scale)) for _ in range(size)]
            at = ", ".join(f"{k}: {nested(v, [len(v)])}" for k, v in axes.items())
            lines += [f"      p{pi}:", f"        at: {{{at}}}", f"        {family}:"]
            lines += [f"          {k}: {nested([d[k] for d in ds], shape)}" for k in ds[0]]
            kind = draw(st.sampled_from([None, "true", "params", "point"]))
            first = next(iter(ds[0]))
            if kind == "true":
                lines.append("        fixed: true")
            elif kind == "params":
                lines.append(f"        fixed: [{first}]")
            elif kind == "point":
                point = ",".join(f"{k}={v[0]!r}" for k, v in axes.items())
                lines.append(f'        fixed: ["@{point}"]')
            if kind:
                holds.append((q, f"p{pi}", kind, axes, first))
    return "\n".join(lines) + "\n", holds


@st.composite
def rows_from(draw, rules: Rules) -> list[dict]:
    """Rows the rules give, at inputs a little beyond their points, a value
    missing now and then, and now and then one row many times over."""
    spans = {}
    for q in rules.quantities:
        for _, _, params in rules.distributions(q, {"a": 0.0, "b": 0.0, "q0": 0.0}):
            spans[q] = max(spans.get(q, 0.0), max(abs(v) for v in params.values()))
    scale = max(spans.values(), default=1.0) or 1.0
    rows = []
    for _ in range(draw(st.integers(0, 50))):
        given_ = {k: draw(st.floats(-1.5, 1.5)) * scale * 100 for k in ("a", "b")}
        quantiles = {q: draw(st.floats(0, 1)) for q in rules.quantities}
        try:
            row = {**given_, **rules.sample(given_, quantiles)}
        except RulesError:
            row = dict(given_)
        rows.append({k: v for k, v in row.items() if draw(st.integers(0, 9))})
    if rows and draw(st.booleans()):
        rows += [rows[0]] * draw(st.integers(1, 20))
    return rows


# The rules and rows fuzzing found bugs with, kept.
ONE_ROW_ON_A_NORMAL_END = (
    "version: 2\nquantities:\n  q0:\n    parts:\n      p0:\n        at: {a: [0.0]}\n"
    "        normal: {mean: [0.0], sd: [1.0], low: [0.0], high: [1.0]}\n",
    [{"a": 0.0, "q0": 0.0}],
)
ONE_ROW_ON_A_TRIANGULAR_END = (
    "version: 2\nquantities:\n  q0:\n    parts:\n      p0:\n        at: {a: [0.0]}\n"
    "        triangular: {low: [0.0], mode: [0.0], high: [0.015625]}\n",
    [{"a": 0.0, "q0": 0.0}],
)
TIED_PARTS = (
    "version: 2\nquantities:\n  q0:\n    combine: max\n    parts:\n      p0:\n        at: {a: [0.0]}\n"
    "        uniform: {low: [0.0], high: [2.0]}\n      p1:\n        at: {a: [0.0]}\n"
    "        uniform: {low: [1.382390089321256e-194], high: [1.0]}\n",
    [{"a": 0.0, "q0": 1.382390089321256e-194}],
)


def check_fit(text: str, rows: list[dict], holds, tmp: Path) -> None:
    rules = Rules.from_text(text)
    try:
        fit = rules.fit(rows, max_iterations=200)
    except RulesError:
        return
    for name, q in fit.quantities.items():
        before, after = q.log_likelihood_before, q.log_likelihood
        assert not math.isnan(after)
        # An ascent ends no lower than the rules as written.
        if math.isfinite(before):
            assert after >= before - 1e-9 * max(1.0, abs(before)), (name, before, after)
        for c in (q.calibration_before, q.calibration):
            assert c.rows == q.rows
            assert all(0 <= v <= 1 for v in (c.in_band, c.outside, c.ks)), c
            assert not c.rows or abs(sum(c.histogram) - 1) < 1e-9, c
    # What the rules hold stays as written.
    for q, part, kind, axes, first in holds:
        points = list(itertools.product(*axes.values()))
        if kind == "point":
            points = points[:1]
        for point in points:
            at = {"a": 0.0, "b": 0.0, **dict(zip(axes, point))}
            was = {n: p for n, _, p in rules.distributions(q, at)}[part]
            now = {n: p for n, _, p in fit.rules.distributions(q, at)}[part]
            for k in [first] if kind == "params" else was:
                assert now[k] == was[k], (q, part, kind, point, k)
    # The fitted file reads back -- over the rules, and extending them -- to
    # the fitted rules, which the report describes.
    over = rules.text_over(fit.text())
    assert all(
        over.distributions(q, {"a": 1.0, "b": -1.0, "q0": 0.5}) == fit.rules.distributions(q, {"a": 1.0, "b": -1.0, "q0": 0.5})
        for q in rules.quantities
    )
    (tmp / "base.yaml").write_text(text)
    (tmp / "sub").mkdir(exist_ok=True)
    fit.write(tmp / "sub" / "fit.yaml", extends=tmp / "base.yaml")
    assert Rules.read(tmp / "sub" / "fit.yaml").quantities == fit.rules.quantities
    if fit.quantities:
        ll = fit.rules.log_likelihood(rows)
        cal = fit.rules.calibration(rows)
        for name, q in fit.quantities.items():
            assert ll[name] == (q.log_likelihood, q.rows)
            assert cal[name] == q.calibration
            for p, _, d in fit.rules.distributions(name, {"a": 0.0, "b": 0.0, "q0": 0.0}):
                assert all(math.isfinite(v) for v in d.values()), (name, p, d)
            summary = fit.rules.evaluate(name, {"a": 0.0, "b": 0.0, "q0": 0.0})
            assert all(math.isfinite(v) for v in summary.values()), (name, summary)


@SETTINGS
@given(data=st.data())
def test_a_fit_holds_its_promises(data, tmp_path_factory) -> None:
    text, holds = data.draw(rule_files())
    try:
        rules = Rules.from_text(text)
    except RulesError:
        return
    check_fit(text, data.draw(rows_from(rules)), holds, tmp_path_factory.mktemp("fit"))


@pytest.mark.parametrize("case", [ONE_ROW_ON_A_NORMAL_END, ONE_ROW_ON_A_TRIANGULAR_END, TIED_PARTS])
def test_what_fuzzing_found_stays_fixed(case, tmp_path: Path) -> None:
    check_fit(case[0], case[1], [], tmp_path)


VALUE = st.one_of(
    st.floats(allow_nan=True, allow_infinity=True),
    st.none(),
    st.integers(-(10**20), 10**20),
    st.booleans(),
    st.sampled_from(["", "1.5", " 2 ", "nan", "inf", "-inf", "1e400", "abc", "1_000", "0x10"]),
)


@SETTINGS
@given(st.data())
def test_any_rows_and_holds_fail_cleanly(data) -> None:
    """Whatever the rows and the `fixed` entries, a fit either works or says
    why (RulesError) -- never a panic, never another error."""
    text, _ = data.draw(rule_files())
    try:
        rules = Rules.from_text(text)
    except RulesError:
        return
    keys = st.sampled_from(["a", "b", "q0", "q1", "other"])
    rows = data.draw(st.lists(st.dictionaries(keys, VALUE, max_size=5), max_size=20))
    fixed = data.draw(st.lists(st.one_of(
        st.sampled_from(["q0", "q1", "q0.p0", "q0.p0.low", "q0.p0.mean", "q0.p1.sd", "q0.p0@a=0", "q0.p0.mode@b=1"]),
        st.text(max_size=12),
    ), max_size=3))
    for call in (
        lambda: rules.fit(rows, fixed=fixed, max_iterations=50),
        lambda: rules.calibration(rows),
        lambda: rules.log_likelihood(rows),
    ):
        try:
            call()
        except RulesError:
            pass


JSON_VALUE = st.recursive(
    st.one_of(st.none(), st.booleans(), st.floats(allow_nan=False), st.integers(), st.text(max_size=5)),
    lambda inner: st.one_of(st.lists(inner, max_size=3), st.dictionaries(st.text(max_size=3), inner, max_size=3)),
    max_leaves=10,
)


@SETTINGS
@given(st.one_of(st.text(max_size=200), JSON_VALUE.map(json.dumps)), st.one_of(st.none(), st.text(max_size=3)))
def test_any_json_data_reads_or_says_why(text, weight) -> None:
    rules = Rules.from_text(
        "version: 2\nquantities:\n  q:\n    parts:\n      p: {at: {a: [0, 1]}, uniform: {low: 0, high: 1}}\n"
    )
    for doc in (text, json.dumps({"rows": json.loads(text) if _is_json(text) else [], "weight": weight})):
        try:
            rows, column = openodd._core.json_rows(doc)
            rules._core.observations(rows, column)
        except ValueError:
            pass


def _is_json(text: str) -> bool:
    try:
        json.loads(text)
    except ValueError:
        return False
    return True


@SETTINGS
@given(st.text(max_size=40))
def test_any_fixed_entry_in_a_file_reads_or_says_why(spec) -> None:
    text = (
        "version: 2\nquantities:\n  q:\n    parts:\n      p:\n        at: {a: [0, 1], b: [0, 1]}\n"
        "        normal: {mean: 0, sd: 1, low: -5, high: 5}\n        fixed: [" + json.dumps(spec) + "]\n"
    )
    try:
        Rules.from_text(text)
    except RulesError:
        pass


@st.composite
def any_dist(draw) -> tuple[str, dict[str, float]]:
    """A distribution of any family at any scale; a normal's mean anywhere,
    up to 200 standard deviations outside its range."""
    family = draw(st.sampled_from(FAMILIES))
    scale = 10.0 ** draw(st.integers(-8, 8))
    low = draw(st.floats(-100, 100)) * scale
    width = draw(st.floats(1e-6, 100)) * scale
    if family == "uniform":
        return family, {"low": low, "high": low + width}
    if family == "triangular":
        return family, {"low": low, "mode": low + draw(st.floats(0, 1)) * width, "high": low + width}
    sd = draw(st.floats(1e-6, 100)) * scale
    return family, {"mean": low + draw(st.floats(-200, 200)) * sd, "sd": sd, "low": low, "high": low + width}


@SETTINGS
@example(("normal", {"mean": -0.605, "sd": 0.0157, "low": 0.0, "high": 1.0}), [0.0, 0.5])
@example(("normal", {"mean": 2295.0, "sd": 15.0, "low": 0.0, "high": 1e-6}), [0.0, 0.5])
@example(("normal", {"mean": 4.75, "sd": 0.125, "low": 0.0, "high": 5.0}), [0.0, 1.0])
@example(("triangular", {"low": 7.547092539411937e-281, "mode": 7.547092539411937e-281, "high": 1.0}), [0.0, 1.0])
@example(("triangular", {"low": 0.0, "mode": 2.22507384e-316, "high": 1e-08}), [0.0, 0.0])
@example(("triangular", {"low": 0.0, "mode": 8.612787633870585e-107, "high": 2000.0}), [4.1072864424437134e-221, 4.306393816935293e-110])
@example(("triangular", {"low": 0.0, "mode": 1.0, "high": 1.0}), [0.0, 1.0])
@example(("triangular", {"low": -1.0, "mode": 0.0, "high": 1e-20}), [0.5, 1.0])
@example(("normal", {"mean": -0.00027044812011718753, "sd": 6.83125e-05, "low": 0.0, "high": 2.9e-05}), [0.0, 2.220446049250313e-16])
@given(any_dist(), st.lists(st.floats(0, 1), min_size=2, max_size=6))
def test_a_distribution_reads_everywhere(dist, ps) -> None:
    """Quantiles rise with p and stay in the range, cdf undoes quantile, and
    every summary is a number in the range -- whatever the distribution."""
    family, params = dist
    body = ", ".join(f"{k}: {v!r}" for k, v in params.items())
    try:
        rules = Rules.from_text(
            f"version: 2\nquantities:\n  q:\n    parts:\n      p: {{at: {{a: [0]}}, {family}: {{{body}}}}}\n"
        )
    except RulesError:
        return
    low, high = params["low"], params["high"]
    ps = sorted(ps)
    xs = [rules.quantile("q", {"a": 0}, p) for p in ps]
    assert all(math.isfinite(x) and low <= x <= high for x in xs), xs
    assert all(a <= b for a, b in zip(xs, xs[1:])), xs
    for p, x in zip(ps, xs):
        c = rules.cdf("q", {"a": 0}, x)
        assert 0 <= c <= 1
        if high - low > 1e-9 * max(abs(low), abs(high)) and 1e-6 < p < 1 - 1e-6:
            again = rules.quantile("q", {"a": 0}, c)
            assert abs(c - p) < 1e-6 or abs(x - again) <= 1e-9 * max(abs(x), high - low), (p, x, c)
    for k, v in rules.evaluate("q", {"a": 0}).items():
        assert math.isfinite(v) and low - 1e-9 * abs(low) <= v <= high + 1e-9 * abs(high), (k, v)
