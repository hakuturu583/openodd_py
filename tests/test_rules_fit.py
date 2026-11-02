"""Fitting rules to data by maximum likelihood (src/rules_fit.rs,
openodd.Rules.fit, openodd-rules-fit)."""

from __future__ import annotations

import csv
import json
import math
import random
from pathlib import Path

import pytest

from openodd import Rules, RulesError, read_rows
from openodd.rules import fit_main

TRUTH = """
version: 2
inputs:
  fog: {default: 0}
quantities:
  grip:
    parts:
      water:
        at: {water: [0, 100]}
        normal: {mean: [0.9, 0.5], sd: [0.05, 0.1], low: 0, high: 2}
  visibility:
    parts:
      fog:
        at: {fog: [0, 100]}
        uniform: {low: [250, 15], high: [400, 40]}
  sight_speed:
    parts:
      sight:
        at: {visibility: [15, 400]}
        triangular: {low: [5, 25], mode: [8, 35], high: [10, 45]}
"""

#: The same structure, its numbers off.
START = (
    TRUTH.replace("mean: [0.9, 0.5], sd: [0.05, 0.1]", "mean: [0.7, 0.7], sd: [0.3, 0.3]")
    .replace("low: [250, 15], high: [400, 40]", "low: [100, 100], high: [200, 200]")
    .replace("mode: [8, 35]", "mode: [6, 40]")
)


def drawn(n: int, seed: int = 7) -> list[dict[str, float]]:
    """*n* rows the truth gives, as a log of drives would hold them."""
    rules = Rules.from_text(TRUTH)
    rng = random.Random(seed)
    rows = []
    for _ in range(n):
        given = {"water": rng.uniform(0, 100), "fog": rng.uniform(0, 100)}
        quantiles = {q: rng.random() for q in rules.quantities}
        rows.append({**given, **rules.sample(given, quantiles)})
    return rows


@pytest.fixture(scope="module")
def rows() -> list[dict[str, float]]:
    return drawn(3000)


class TestFit:
    def test_every_factor_recovers_its_parameters(self, rows) -> None:
        fit = Rules.from_text(START).fit(rows)
        assert list(fit.quantities) == ["grip", "visibility", "sight_speed"]
        for q in fit.quantities.values():
            assert q.rows == 3000 and q.skipped == 0 and q.converged
            assert q.log_likelihood > q.log_likelihood_before
            assert q.parts[0].points_with_data == q.parts[0].points == 2
        rules = fit.rules
        grip = dict(rules.distributions("grip", {"water": 0})[0][2])
        assert grip["mean"] == pytest.approx(0.9, abs=0.01)
        assert grip["sd"] == pytest.approx(0.05, abs=0.01)
        assert (grip["low"], grip["high"]) == (0.0, 2.0)  # the truncation is held
        assert rules.range("visibility", {"fog": 0}) == pytest.approx((250, 400), abs=3)
        assert rules.range("visibility", {"fog": 100}) == pytest.approx((15, 40), abs=1)
        at_far = dict(rules.distributions("sight_speed", {"visibility": 400})[0][2])
        assert at_far == pytest.approx({"low": 25, "mode": 35, "high": 45}, abs=1.0)

    def test_the_fit_is_at_least_as_likely_as_the_truth(self, rows) -> None:
        fit = Rules.from_text(START).fit(rows)
        truth = Rules.from_text(TRUTH).log_likelihood(rows)
        for name, q in fit.quantities.items():
            assert truth[name][1] == q.rows
            assert q.log_likelihood >= truth[name][0] - 1e-6
            assert fit.rules.log_likelihood(rows)[name][0] == pytest.approx(q.log_likelihood)

    def test_calibration_says_how_well_the_rules_match(self, rows) -> None:
        start = Rules.from_text(START)
        before = start.calibration(rows)
        fit = start.fit(rows)
        for name, q in fit.quantities.items():
            assert q.calibration_before == before[name]
            assert q.calibration.rows == q.rows
            assert len(q.calibration.histogram) == 10
            assert sum(q.calibration.histogram) == pytest.approx(1.0)
            # The fitted rules match: 90% in the band, u spread evenly.
            assert q.calibration.in_band == pytest.approx(0.9, abs=0.03), name
            assert q.calibration.ks < 0.05, name
        # The start's grip was too wide: more than 90% in its band.
        assert before["grip"].in_band > 0.95
        assert fit.rules.calibration(rows)["grip"] == fit.quantities["grip"].calibration

    def test_missing_values_drop_a_row_from_the_factors_that_need_them(self, rows) -> None:
        holed = [dict(r) for r in rows[:900]]
        for r in holed[::3]:
            r["visibility"] = None  # not measured
        for r in holed[1::3]:
            r["water"] = float("nan")
        fit = Rules.from_text(START).fit(holed)
        q = fit.quantities
        assert (q["visibility"].rows, q["visibility"].skipped) == (600, 0)
        assert (q["sight_speed"].rows, q["sight_speed"].skipped) == (600, 300)
        assert (q["grip"].rows, q["grip"].skipped) == (600, 300)
        # fog is missing nowhere, but would read its default (0) if it were.
        no_fog = [{k: v for k, v in r.items() if k != "fog"} for r in rows[:50]]
        assert Rules.from_text(START).fit(no_fog, quantities=["visibility"]).quantities[
            "visibility"
        ].rows == 50

    def test_only_the_quantities_named_are_fitted(self, rows) -> None:
        fit = Rules.from_text(START).fit(rows, quantities=["grip"])
        assert list(fit.quantities) == ["grip"]
        start = Rules.from_text(START)
        assert fit.rules.range("visibility", {"fog": 0}) == start.range("visibility", {"fog": 0})

    def test_weights_count_rows(self, rows) -> None:
        rules = Rules.from_text(START)
        doubled = rules.fit(rows[:400] + rows[:400], quantities=["grip"])
        weighted = rules.fit(rows[:400], [2.0] * 400, quantities=["grip"])
        assert weighted.quantities["grip"].log_likelihood == pytest.approx(
            doubled.quantities["grip"].log_likelihood, rel=1e-6
        )
        with pytest.raises(RulesError, match="3 weights for 400 rows"):
            rules.fit(rows[:400], [1.0, 1.0, 1.0])
        with pytest.raises(RulesError, match="weight is -1.0"):
            rules.fit(rows[:2], [1.0, -1.0])

    def test_the_fitted_file_reads_back_as_the_fitted_rules(self, rows, tmp_path: Path) -> None:
        base = tmp_path / "rules" / "weather.yaml"
        base.parent.mkdir()
        base.write_text(START)
        fit = Rules.read(base).fit(rows)
        out = tmp_path / "fitted" / "weather_fitted.yaml"
        out.parent.mkdir()
        fit.write(out, extends=base)
        assert out.read_text().startswith('version: 2\nextends: "../rules/weather.yaml"\n')
        again = Rules.read(out)
        for q in again.quantities:
            for given in ({"water": 30, "fog": 60}, {"water": 90, "fog": 5}):
                assert again.evaluate(q, given) == pytest.approx(fit.rules.evaluate(q, given))
        # Without extends, it merges over the rules fitted.
        over = Rules.read(base).text_over(fit.text())
        assert over.evaluate("grip", {"water": 30}) == pytest.approx(
            fit.rules.evaluate("grip", {"water": 30})
        )

    def test_values_are_read_as_numbers_and_other_columns_ignored(self) -> None:
        rules = Rules.from_text(START)
        fit = rules.fit(
            [{"time": "12:00:01", "water": "10", "grip": " 0.85 ", "fog": ""}] * 3
            + [{"water": 20, "grip": 0.83}] * 3,
            quantities=["grip"],
        )
        assert fit.quantities["grip"].rows == 6
        with pytest.raises(RulesError, match='row 1: grip is "wet", not a number'):
            rules.fit([{"water": 1, "grip": 1}, {"water": 1, "grip": "wet"}])

    def test_a_row_outside_a_normals_truncation_is_refused(self) -> None:
        with pytest.raises(RulesError, match="outside the normal distribution's truncation"):
            Rules.from_text(START).fit([{"water": 10, "grip": 2.5}])

    def test_a_quantity_named_that_no_row_gives_is_refused(self) -> None:
        with pytest.raises(RulesError, match="cannot fit sight_speed: no row gives it"):
            Rules.from_text(START).fit([{"water": 1}], quantities=["sight_speed"])
        with pytest.raises(RulesError, match="unknown quantity 'nope'"):
            Rules.from_text(START).fit([{"water": 1}], quantities=["nope"])

    def test_a_degenerate_quantity_is_held_wide(self) -> None:
        # Every row the same value: the likelihood would grow without bound
        # as sd shrinks; min_spread holds it.
        rows = [{"water": 0, "grip": 0.8}] * 20
        fit = Rules.from_text(START).fit(rows, quantities=["grip"], min_spread=0.01)
        sd = dict(fit.rules.distributions("grip", {"water": 0})[0][2])["sd"]
        assert sd == pytest.approx(0.01 * 0.8, rel=1e-3)
        assert math.isfinite(fit.quantities["grip"].log_likelihood)


class TestFixed:
    def test_the_rule_file_holds_what_must_not_move(self, rows) -> None:
        held = START.replace(
            "        normal: {mean: [0.7, 0.7], sd: [0.3, 0.3], low: 0, high: 2}",
            "        normal: {mean: [0.7, 0.7], sd: [0.3, 0.3], low: 0, high: 2}\n"
            "        fixed: [sd]",
        ).replace(
            "        uniform: {low: [100, 100], high: [200, 200]}",
            "        uniform: {low: [100, 100], high: [200, 200]}\n        fixed: true",
        )
        rules = Rules.from_text(held)
        fit = rules.fit(rows)
        # visibility is held whole: not fitted.
        assert list(fit.quantities) == ["grip", "sight_speed"]
        assert fit.rules.range("visibility", {"fog": 50}) == rules.range("visibility", {"fog": 50})
        grip = dict(fit.rules.distributions("grip", {"water": 0})[0][2])
        assert grip["sd"] == 0.3
        assert grip["mean"] == pytest.approx(0.9, abs=0.03)
        assert fit.quantities["grip"].parts[0].estimated == 2  # the mean at two points

    def test_a_fit_holds_more(self, rows) -> None:
        rules = Rules.from_text(START)
        fit = rules.fit(
            rows,
            fixed=["visibility", "grip.water.mean@water=100", "sight_speed.sight@visibility=15"],
        )
        assert list(fit.quantities) == ["grip", "sight_speed"]
        assert dict(fit.rules.distributions("grip", {"water": 100})[0][2])["mean"] == 0.7
        assert fit.rules.distributions("sight_speed", {"visibility": 15}) == rules.distributions(
            "sight_speed", {"visibility": 15}
        )
        assert [p.estimated for p in fit.quantities["sight_speed"].parts] == [3]
        with pytest.raises(RulesError, match="fixed 'grip.ice': grip has no part 'ice'"):
            rules.fit(rows, fixed=["grip.ice"])

    def test_a_range_held_too_narrow_is_refused(self) -> None:
        with pytest.raises(RulesError, match="where its low or high is held"):
            Rules.from_text(START).fit(
                [{"fog": 0, "visibility": 300}], fixed=["visibility.fog.high"]
            )


class TestReadRows:
    def test_csv_jsonl_and_json(self, tmp_path: Path) -> None:
        (tmp_path / "a.csv").write_text("water,grip\n10,0.8\n20,\n")
        assert read_rows(tmp_path / "a.csv") == [
            {"water": "10", "grip": "0.8"},
            {"water": "20", "grip": ""},
        ]
        (tmp_path / "a.jsonl").write_text('{"water": 1}\n\n{"grip": 0.5}\n')
        assert read_rows(tmp_path / "a.jsonl") == [{"water": 1}, {"grip": 0.5}]
        (tmp_path / "a.json").write_text(json.dumps({"water": [1, 2], "grip": [0.9, None]}))
        assert read_rows(tmp_path / "a.json") == [
            {"water": 1, "grip": 0.9},
            {"water": 2, "grip": None},
        ]
        (tmp_path / "b.json").write_text(
            json.dumps({"rows": [{"water": 1, "dt": 0.5}, {"grip": 0.5}], "weight": "dt"})
        )
        assert read_rows(tmp_path / "b.json") == [{"water": 1, "dt": 0.5}, {"grip": 0.5}]
        from openodd.rules import read_data

        assert read_data(tmp_path / "b.json")[1] == "dt"
        (tmp_path / "c.json").write_text(json.dumps({"rows": [], "weights": "dt"}))
        with pytest.raises(RulesError, match='unknown key "weights"'):
            read_rows(tmp_path / "c.json")
        (tmp_path / "a.txt").write_text("")
        with pytest.raises(RulesError, match="data is read from"):
            read_rows(tmp_path / "a.txt")


class TestCommand:
    def test_a_json_file_names_its_weight(self, rows, tmp_path: Path, capsys) -> None:
        (tmp_path / "weather.yaml").write_text(START)
        data = [{**r, "dt": 2.0} for r in rows[:300]] + [dict(r) for r in rows[300:600]]
        (tmp_path / "d.json").write_text(json.dumps({"rows": data, "weight": "dt"}))
        out = tmp_path / "o.yaml"
        assert fit_main([str(tmp_path / "weather.yaml"), str(tmp_path / "d.json"), "-o", str(out), "-q", "grip"]) == 0
        weights = [2.0] * 300 + [1.0] * 300
        want = Rules.from_text(START).fit(rows[:600], weights, quantities=["grip"])
        got = Rules.read(out)
        assert got.evaluate("grip", {"water": 40}) == pytest.approx(
            want.rules.evaluate("grip", {"water": 40}), rel=1e-6
        )

    def test_fits_a_file_to_csv_logs(self, rows, tmp_path: Path, capsys) -> None:
        (tmp_path / "weather.yaml").write_text(START)
        for i, chunk in enumerate((rows[:1500], rows[1500:])):
            with (tmp_path / f"drive{i}.csv").open("w", newline="") as f:
                w = csv.DictWriter(f, fieldnames=["water", "fog", "grip", "visibility", "sight_speed", "w"])
                w.writeheader()
                for r in chunk:
                    w.writerow({**r, "w": 1})
        out = tmp_path / "fitted.yaml"
        code = fit_main(
            [
                str(tmp_path / "weather.yaml"),
                str(tmp_path / "drive0.csv"),
                str(tmp_path / "drive1.csv"),
                "-o",
                str(out),
                "-q",
                "grip",
                "--weight",
                "w",
                "--fix",
                "grip.water.sd",
            ]
        )
        assert code == 0
        printed = capsys.readouterr().out
        assert "3000 rows" in printed
        assert "grip: 3000 rows (0 skipped)" in printed
        assert "water: 2 of 2 points with data, 2 values estimated" in printed
        assert "in the central 90%:" in printed
        fitted = Rules.read(out)
        assert dict(fitted.distributions("grip", {"water": 100})[0][2])["sd"] == 0.3

    def test_an_error_is_reported(self, tmp_path: Path, capsys) -> None:
        (tmp_path / "weather.yaml").write_text(START)
        (tmp_path / "d.csv").write_text("water,grip\n1,wet\n")
        assert fit_main([str(tmp_path / "weather.yaml"), str(tmp_path / "d.csv"), "-o", str(tmp_path / "o.yaml")]) == 2
        assert 'grip is "wet", not a number' in capsys.readouterr().err
