"""Coverage files as a test report: what passed, what failed, and why.

:class:`TestReport` reads the coverage files of a campaign (what
:class:`~.collector.CoverageCollector` writes per run) and lays them out as
one standalone HTML page a reviewer reads top to bottom:

* the verdict -- runs, passed, failed, pass rate, ODD coverage, flaky cells;
* an analysis -- what Claude found when it went through the merged report
  with the Claude Agent SDK (:mod:`openodd.agent`), when one was made;
* results per scenario and environment, and every failed run with its case
  and where it failed;
* ODD coverage per item, with its holes; the flaky ODD cells and cases;
* the interactive views: the ODD matrix (:class:`~.view.CoverageView`), the
  drawn cases (:class:`~.view.CasePlot`) and the scenario's realisation.

Every number on the page is the report's own; the analysis only reads them
(:meth:`TestReport.summary` is what Claude is given to start from)::

    report = openodd.TestReport.from_paths(["runs/"])
    analysis = openodd.agent.analyze(report)      # optional; needs claude-agent-sdk
    Path("report.html").write_text(report.page(analysis=analysis))

``openodd-test-report runs/ -o report.html`` does the same from the shell.
"""

from __future__ import annotations

import argparse
import json
import logging
import os
import sys
from collections.abc import Iterable, Mapping, Sequence
from dataclasses import asdict, dataclass, field
from html import escape
from pathlib import Path
from typing import Any, Optional

from .report import CoverageReport, load_coverage_files, merge_coverage
from .view import CasePlot, CoverageView, page

__all__ = ["Finding", "ReportAnalysis", "TestReport", "main"]

logger = logging.getLogger(__name__)

#: A case is listed as flaky when the report's reassessment is this confident.
FLAKY_CONFIDENCE = 0.5
#: Severities a finding may have, most severe first.
SEVERITIES = ("critical", "major", "minor", "info")
#: Verdicts an analysis may give the campaign.
VERDICTS = ("pass", "fail", "inconclusive")


@dataclass
class Finding:
    """One thing the analysis found."""

    severity: str
    title: str
    detail: str
    #: The figures it rests on, as quoted from the report.
    evidence: list[str] = field(default_factory=list)


@dataclass
class ReportAnalysis:
    """What an analyst -- Claude, through :mod:`openodd.agent`, or a person --
    concluded from a :class:`TestReport`."""

    verdict: str
    headline: str
    summary: str
    findings: list[Finding] = field(default_factory=list)
    recommendations: list[str] = field(default_factory=list)
    #: Who made it: a model id, or a name.
    author: str = ""
    #: What it cost, when the analyst reported it.
    cost_usd: Optional[float] = None

    @classmethod
    def from_dict(cls, raw: Mapping[str, Any]) -> "ReportAnalysis":
        verdict = str(raw.get("verdict", "inconclusive")).lower()
        findings = []
        for f in raw.get("findings") or []:
            severity = str(f.get("severity", "info")).lower()
            findings.append(Finding(
                severity=severity if severity in SEVERITIES else "info",
                title=str(f.get("title", "")),
                detail=str(f.get("detail", "")),
                evidence=[str(e) for e in f.get("evidence") or []],
            ))
        cost = raw.get("cost_usd")
        return cls(
            verdict=verdict if verdict in VERDICTS else "inconclusive",
            headline=str(raw.get("headline", "")),
            summary=str(raw.get("summary", "")),
            findings=findings,
            recommendations=[str(r) for r in raw.get("recommendations") or []],
            author=str(raw.get("author", "")),
            cost_usd=None if cost is None else float(cost),
        )

    def to_dict(self) -> dict[str, Any]:
        return asdict(self)


def _rate(passed: int, failed: int) -> Optional[float]:
    return passed / (passed + failed) if passed + failed else None


class TestReport:
    """The coverage documents of a campaign, merged, as a test report.

    Args:
        documents: The contents of the coverage files.
        names: A name per document (its file, say) for the failed runs'
            table; its index when left out.
        title: The page's title.
    """

    __test__ = False  # not a pytest class, despite its name

    def __init__(self, documents: Sequence[Mapping[str, Any]], *,
                 names: Optional[Sequence[str]] = None, title: str = "ODD test report") -> None:
        self.documents = [dict(d) for d in documents]
        if names is not None and len(names) != len(self.documents):
            raise ValueError(f"{len(names)} names for {len(self.documents)} documents")
        self.names = list(names) if names is not None else [f"#{i}" for i in range(len(self.documents))]
        self.title = title
        self.report: CoverageReport = merge_coverage(self.documents)
        self._summary: Optional[dict[str, Any]] = None

    @classmethod
    def from_paths(cls, paths: Iterable[Path], **kwargs: Any) -> "TestReport":
        """The coverage files under *paths* (files, or directories searched)."""
        loaded = load_coverage_files(paths)
        files = [p.resolve() for p, _ in loaded]
        root = Path(os.path.commonpath([f.parent for f in files])) if files else Path()
        return cls([doc for _, doc in loaded], names=[f.relative_to(root).as_posix() for f in files], **kwargs)

    # -- the figures --------------------------------------------------------

    def summary(self) -> dict[str, Any]:
        """The report's figures as plain data: the verdict counts, results per
        scenario and environment, ODD exposure and coverage, every failed run,
        the flaky cells and the flaky cases."""
        if self._summary is None:
            self._summary = self._compute_summary()
        return self._summary

    def _compute_summary(self) -> dict[str, Any]:
        per_scenario: dict[str, dict[str, int]] = {}
        per_environment: dict[str, dict[str, int]] = {}
        failures = []
        for i, doc in enumerate(self.documents):
            outcome = doc.get("outcome") or {}
            passed = outcome.get("passed")
            key = "no_outcome" if passed is None else "passed" if passed else "failed"
            scenario = str(doc.get("scenario", ""))
            environment = outcome.get("environment")
            for table, name in ((per_scenario, scenario), (per_environment, environment)):
                if table is per_environment and passed is None:
                    continue
                row = table.setdefault("" if name is None else str(name),
                                       {"runs": 0, "passed": 0, "failed": 0, "no_outcome": 0})
                row["runs"] += 1
                row[key] += 1
            if passed is False:
                case = doc.get("case") or {}
                failures.append({
                    "document": i,
                    "name": self.names[i],
                    "scenario": scenario,
                    "environment": environment,
                    "failed_at": outcome.get("failed_at"),
                    "failed_cell": outcome.get("failed_cell"),
                    "values": case.get("values"),
                    "chosen_for": case.get("chosen_for"),
                })
        passed = sum(r["passed"] for r in per_scenario.values())
        failed = sum(r["failed"] for r in per_scenario.values())

        def rows(table: dict[str, dict[str, int]], key: str) -> list[dict[str, Any]]:
            return [{key: name, **row, "pass_rate": _rate(row["passed"], row["failed"])}
                    for name, row in table.items()]

        groups = {g: self.report.group_grade(g) for g in self.report.groups()}
        items = [{
            "name": e.name, "group": e.group, "event": e.event, "unit": e.unit,
            "grade": e.grade, "covered": len(e.covered), "targets": len(e.targets),
            "holes": list(e.holes),
        } for e in self.report.entries]
        odds = [{
            "name": o.name, "runs": o.runs, "runs_outside": o.runs_outside,
            "seconds": o.seconds, "revisions": o.revisions, "unmeasured": o.unmeasured,
        } for o in self.report.odds.values()]
        flaky_cells = [{
            "scenario": g.scenario, "environment": g.environment,
            "attributes": g.attributes, "labels": list(r.labels),
            "runs_visited": r.runs_visited, "runs_failed": r.runs_failed,
            "fail_rate": r.fail_rate, "ci": [r.ci_low, r.ci_high], "fail_at_cell": r.fail_at_cell,
        } for g, r in self.report.flaky_cells()]
        failing_cells = [{
            "scenario": g.scenario, "environment": g.environment,
            "attributes": g.attributes, "labels": list(r.labels),
            "runs_visited": r.runs_visited, "fail_at_cell": r.fail_at_cell,
        } for g in self.report.cell_flakiness for r in g.failing_cells()]
        cases = [c for c in self.report.cases
                 if c.assessed and c.assessed.get("instability_confidence") is not None]
        cases.sort(key=lambda c: -float(c.assessed["instability_confidence"]))  # type: ignore[index]
        flaky_cases = [{
            "document": c.document, "scenario": c.scenario, "values": c.values,
            "chosen_for": c.chosen_for, "passed": c.passed, **(c.assessed or {}),
        } for c in cases if float(c.assessed["instability_confidence"]) > FLAKY_CONFIDENCE]  # type: ignore[index]
        return {
            "title": self.title,
            "runs": len(self.documents),
            "passed": passed,
            "failed": failed,
            "no_outcome": len(self.documents) - passed - failed,
            "pass_rate": _rate(passed, failed),
            "scenarios": rows(per_scenario, "scenario"),
            "environments": rows(per_environment, "environment"),
            "odds": odds,
            "coverage": {"groups": groups, "items": items},
            "failures": failures,
            "flaky_cells": flaky_cells,
            "failing_cells": failing_cells,
            "flaky_cases": flaky_cases,
            "cases_recorded": len(self.report.cases),
        }

    def markdown(self, *, max_holes: int = 20) -> str:
        """The merged coverage report as Markdown (``openodd-coverage``'s)."""
        return self.report.to_markdown(max_holes=max_holes)

    # -- the page -----------------------------------------------------------

    def html(self, *, analysis: Optional[ReportAnalysis] = None, views: bool = True,
             cases_x: Optional[str] = None, cases_y: Optional[str] = None) -> str:
        """The report as an HTML fragment (needs :func:`~.view.styles` and
        :func:`~.view.script` on the page for its views); *views* adds the
        interactive ODD matrix, case plot and realisation."""
        s = self.summary()
        parts = [f"<style>{_CSS}</style>", '<div class="oo-cov oo-report">',
                 _header(s), _analysis(analysis), _results(s), _failures(s),
                 _coverage(s), _flaky(s)]
        if views:
            parts.append(self._views(cases_x, cases_y))
        parts.append("</div>")
        return "".join(p for p in parts if p)

    def page(self, *, analysis: Optional[ReportAnalysis] = None, **kwargs: Any) -> str:
        """A standalone page: :meth:`html` with the styles and the script."""
        return page(self.title, self.html(analysis=analysis, **kwargs))

    def _views(self, cases_x: Optional[str], cases_y: Optional[str]) -> str:
        out = []
        matrix = CoverageView(self.documents)
        if matrix.runs:
            out.append(_section("ODD matrix", matrix.explorer_html(depth=2, id="oo-report-matrix")))
        plot = CasePlot(self.documents)
        if plot.cases:
            x = cases_x if cases_x in plot.attributes else None
            y = cases_y if cases_y in plot.attributes else None
            out.append(_section("Drawn cases", plot.explorer_html(x=x, y=y, id="oo-report-cases")))
            out.append(_section("Scenario realisation", plot.drift_html(id="oo-report-drift")))
        return "".join(out)


# -- HTML pieces ----------------------------------------------------------------


def _pct(value: Optional[float]) -> str:
    return "–" if value is None else f"{100 * value:.1f}%"


def _num(value: Any) -> str:
    if isinstance(value, float):
        return f"{value:.6g}"
    return escape(str(value))


def _section(title: str, body: str, *, note: str = "") -> str:
    note_html = f'<p class="oo-r-note">{escape(note)}</p>' if note else ""
    return (f'<section class="oo-r-section"><h2>{escape(title)}</h2>{note_html}{body}</section>')


def _table(head: Sequence[str], rows: Iterable[Sequence[str]], *, cls: str = "") -> str:
    """A table of already-escaped cells."""
    body = "".join("<tr>" + "".join(f"<td>{c}</td>" for c in row) + "</tr>" for row in rows)
    th = "".join(f"<th>{escape(h)}</th>" for h in head)
    return (f'<div class="oo-r-scroll"><table class="oo-r-table {cls}"><thead><tr>{th}</tr></thead>'
            f"<tbody>{body}</tbody></table></div>")


def _values(values: Optional[Mapping[str, Any]]) -> str:
    if not values:
        return "–"
    return ", ".join(f"{escape(str(k))}={_num(v)}" for k, v in values.items())


def _bar(grade: Optional[float]) -> str:
    if grade is None:
        return "–"
    width = max(0.0, min(1.0, grade)) * 100
    tone = "pass" if grade >= 0.999 else "flaky" if grade >= 0.5 else "fail"
    return (f'<span class="oo-r-bar"><span class="oo-r-bar-fill oo-r-{tone}" style="width:{width:.1f}%"></span>'
            f'</span><span class="oo-r-bar-label">{_pct(grade)}</span>')


def _badge(text: str, tone: str) -> str:
    return f'<span class="oo-r-badge oo-r-{tone}">{escape(text)}</span>'


def _tile(label: str, value: str, tone: str = "", sub: str = "") -> str:
    sub_html = f'<div class="oo-r-tile-sub">{escape(sub)}</div>' if sub else ""
    return (f'<div class="oo-r-tile {("oo-r-" + tone) if tone else ""}"><div class="oo-r-tile-label">'
            f'{escape(label)}</div><div class="oo-r-tile-value">{escape(value)}</div>{sub_html}</div>')


def _header(s: Mapping[str, Any]) -> str:
    rate = s["pass_rate"]
    status = ("no outcome recorded" if rate is None else "all runs passed" if s["failed"] == 0
              else f"{s['failed']} run(s) failed")
    tone = "unknown" if rate is None else "pass" if s["failed"] == 0 else "fail"
    groups = s["coverage"]["groups"]
    grade = groups.get("odd", next(iter(groups.values()), None)) if groups else None
    tiles = "".join([
        _tile("Runs", str(s["runs"]), sub=", ".join(f"{r['scenario']}: {r['runs']}" for r in s["scenarios"])[:80]),
        _tile("Passed", str(s["passed"]), "pass"),
        _tile("Failed", str(s["failed"]), "fail" if s["failed"] else ""),
        _tile("Pass rate", _pct(rate), tone, sub=f"{s['no_outcome']} without an outcome" if s["no_outcome"] else ""),
        _tile("ODD coverage", _pct(grade), sub="odd group" if "odd" in groups else ""),
        _tile("Flaky cells", str(len(s["flaky_cells"])), "flaky" if s["flaky_cells"] else ""),
        _tile("Flaky cases", str(len(s["flaky_cases"])), "flaky" if s["flaky_cases"] else "",
              sub=f"of {s['cases_recorded']} recorded" if s["cases_recorded"] else ""),
    ])
    return (f'<header class="oo-r-header"><div><h1>{escape(s["title"])}</h1>'
            f'<p class="oo-r-status">{_badge(status, tone)}</p></div></header>'
            f'<div class="oo-r-tiles">{tiles}</div>')


def _analysis(a: Optional[ReportAnalysis]) -> str:
    if a is None:
        return _section("Analysis", '<p class="oo-r-muted">No analysis was made for this report. '
                        "<code>openodd-test-report</code> without <code>--no-agent</code> has Claude "
                        "go through it (needs <code>claude-agent-sdk</code>).</p>")
    tone = {"pass": "pass", "fail": "fail"}.get(a.verdict, "unknown")
    by = escape(a.author) if a.author else "an analyst"
    cost = f" · ${a.cost_usd:.2f}" if a.cost_usd is not None else ""
    findings = "".join(
        f'<li class="oo-r-finding">{_badge(f.severity, _SEVERITY_TONE.get(f.severity, "unknown"))} '
        f"<strong>{escape(f.title)}</strong><p>{escape(f.detail)}</p>"
        + ("<ul class=\"oo-r-evidence\">" + "".join(f"<li>{escape(e)}</li>" for e in f.evidence) + "</ul>"
           if f.evidence else "")
        + "</li>"
        for f in sorted(a.findings, key=lambda f: SEVERITIES.index(f.severity)))
    recs = "".join(f"<li>{escape(r)}</li>" for r in a.recommendations)
    body = (f'<div class="oo-r-analysis"><p class="oo-r-headline">{_badge(a.verdict, tone)} '
            f"{escape(a.headline)}</p><p>{escape(a.summary)}</p>"
            + (f"<h3>Findings</h3><ul class=\"oo-r-findings\">{findings}</ul>" if findings else "")
            + (f"<h3>Recommendations</h3><ol>{recs}</ol>" if recs else "")
            + f'<p class="oo-r-note">Analysis by {by}{escape(cost)}. Figures above and below are the '
            "report's own.</p></div>")
    return _section("Analysis", body)


_SEVERITY_TONE = {"critical": "fail", "major": "fail", "minor": "flaky", "info": "unknown"}


def _results(s: Mapping[str, Any]) -> str:
    def rows(table: Sequence[Mapping[str, Any]], key: str) -> list[list[str]]:
        return [[escape(str(r[key]) or "(none)"), str(r["runs"]), str(r["passed"]), str(r["failed"]),
                 str(r["no_outcome"]), _bar(r["pass_rate"])] for r in table]

    head = ["Runs", "Passed", "Failed", "No outcome", "Pass rate"]
    body = _table(["Scenario", *head], rows(s["scenarios"], "scenario"))
    if s["environments"]:
        body += "<h3>By environment</h3>" + _table(["Environment", *head], rows(s["environments"], "environment"))
    for odd in s["odds"]:
        secs = odd["seconds"]
        total = sum(secs.values()) or 0.0
        share = (lambda k: _pct(secs.get(k, 0.0) / total)) if total else (lambda k: "–")
        body += (f'<p class="oo-r-note">ODD <strong>{escape(odd["name"])}</strong>: {odd["runs"]} runs, '
                 f'{odd["runs_outside"]} left it; time inside {share("inside")}, inside only because '
                 f'values were missing {share("assumed")}, outside {share("outside")}.</p>')
    return _section("Results", body)


def _failures(s: Mapping[str, Any]) -> str:
    if not s["failures"]:
        return ""
    rows = [[escape(f["name"]), escape(f["scenario"]), escape(str(f["environment"] or "–")),
             _values(f["values"]), "–" if f["failed_at"] is None else f"{f['failed_at']:g} s",
             escape(" · ".join(map(str, f["failed_cell"]))) if f["failed_cell"] else "–",
             escape(str(f["chosen_for"] or "–"))] for f in s["failures"]]
    return _section(f"Failed runs ({len(rows)})", _table(
        ["Run", "Scenario", "Environment", "Case", "Failed at", "Failed in cell", "Chosen for"], rows))


def _coverage(s: Mapping[str, Any]) -> str:
    items = s["coverage"]["items"]
    if not items:
        return ""
    groups = "".join(f"<li>{escape(g)}: {_pct(v)}</li>" for g, v in s["coverage"]["groups"].items())
    rows = [[escape(i["name"]) + (f' <span class="oo-r-muted">[{escape(i["unit"])}]</span>' if i["unit"] else ""),
             escape(i["event"]), _bar(i["grade"]), f"{i['covered']}/{i['targets']}",
             escape(", ".join(i["holes"][:12]) + (" …" if len(i["holes"]) > 12 else "")) or "–"]
            for i in items]
    return _section("ODD coverage", f'<ul class="oo-r-groups">{groups}</ul>'
                    + _table(["Item", "Event", "Grade", "Covered", "Holes"], rows))


def _flaky(s: Mapping[str, Any]) -> str:
    body = ""
    if s["flaky_cells"]:
        rows = [[escape(c["scenario"]), escape(str(c["environment"] or "–")),
                 escape(", ".join(f"{a}={l}" for a, l in zip(c["attributes"], c["labels"]))),
                 str(c["runs_visited"]), str(c["runs_failed"]), _pct(c["fail_rate"]),
                 f"{_pct(c['ci'][0])} – {_pct(c['ci'][1])}", str(c["fail_at_cell"])] for c in s["flaky_cells"]]
        body += "<h3>Flaky ODD cells</h3>" + _table(
            ["Scenario", "Environment", "Cell", "Runs", "Failed", "Fail rate", "95% CI", "Failed here"], rows)
    if s["failing_cells"]:
        rows = [[escape(c["scenario"]), escape(str(c["environment"] or "–")),
                 escape(", ".join(f"{a}={l}" for a, l in zip(c["attributes"], c["labels"]))),
                 str(c["runs_visited"]), str(c["fail_at_cell"])] for c in s["failing_cells"]]
        body += "<h3>Always failing ODD cells</h3>" + _table(
            ["Scenario", "Environment", "Cell", "Runs", "Failed here"], rows)
    if s["flaky_cases"]:
        rows = [[escape(c["scenario"]), _values(c["values"]), escape(str(c["chosen_for"])),
                 "–" if c["passed"] is None else ("yes" if c["passed"] else "no"),
                 _num(c.get("instability_confidence")), _num(c.get("flip_probability")),
                 _num(c.get("pass_probability")), _num(c.get("robustness_radius"))] for c in s["flaky_cases"]]
        body += "<h3>Flaky cases</h3>" + _table(
            ["Scenario", "Case", "Chosen for", "Passed", "Confidence", "Flip", "P(pass)", "Radius"], rows)
    if not body:
        return ""
    return _section("Flakiness", body, note="Cells: the 95% interval of the failure rate lies inside the "
                    "flaky band. Cases: instability confidence above 0.5, refitted over every run.")


_CSS = """
.oo-report{font-family:var(--oo-font);color:var(--oo-ink);max-width:1200px;margin:0 auto;display:flex;flex-direction:column;gap:20px}
.oo-report h1{font-size:26px;margin:0 0 6px}
.oo-report h2{font-size:18px;margin:0 0 12px}
.oo-report h3{font-size:14px;margin:18px 0 8px;color:var(--oo-ink-2)}
.oo-report code{font-family:var(--oo-mono);font-size:12px}
.oo-r-section{background:var(--oo-surface);border:1px solid var(--oo-border);border-radius:var(--oo-radius);padding:18px 20px;box-shadow:var(--oo-shadow);min-width:0}
.oo-r-tiles{display:grid;grid-template-columns:repeat(auto-fit,minmax(140px,1fr));gap:12px}
.oo-r-tile{background:var(--oo-surface);border:1px solid var(--oo-border);border-radius:var(--oo-radius);padding:12px 14px;box-shadow:var(--oo-shadow);border-top:4px solid var(--oo-border-strong)}
.oo-r-tile.oo-r-pass{border-top-color:var(--oo-pass)}.oo-r-tile.oo-r-fail{border-top-color:var(--oo-fail)}
.oo-r-tile.oo-r-flaky{border-top-color:var(--oo-flaky)}.oo-r-tile.oo-r-unknown{border-top-color:var(--oo-unknown)}
.oo-r-tile-label{font-size:12px;color:var(--oo-ink-2)}
.oo-r-tile-value{font-size:24px;font-weight:650;font-variant-numeric:tabular-nums}
.oo-r-tile-sub{font-size:11px;color:var(--oo-ink-3);overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.oo-r-badge{display:inline-block;font-size:12px;font-weight:600;border-radius:999px;padding:2px 10px;text-transform:uppercase;letter-spacing:.03em}
.oo-r-badge.oo-r-pass{background:var(--oo-pass-fill);color:var(--oo-pass)}
.oo-r-badge.oo-r-fail{background:var(--oo-fail-fill);color:var(--oo-fail)}
.oo-r-badge.oo-r-flaky{background:var(--oo-flaky-fill);color:var(--oo-flaky-ink)}
.oo-r-badge.oo-r-unknown{background:var(--oo-unknown-fill);color:var(--oo-ink-2)}
.oo-r-status{margin:0}
.oo-r-scroll{overflow-x:auto}
.oo-r-table{border-collapse:collapse;width:100%;font-size:13px}
.oo-r-table th{text-align:left;font-weight:600;color:var(--oo-ink-2);border-bottom:1px solid var(--oo-border-strong);padding:6px 8px;white-space:nowrap}
.oo-r-table td{border-bottom:1px solid var(--oo-border);padding:6px 8px;vertical-align:top;font-variant-numeric:tabular-nums}
.oo-r-bar{display:inline-block;width:80px;height:8px;border-radius:4px;background:var(--oo-surface-2);border:1px solid var(--oo-border);vertical-align:middle;overflow:hidden}
.oo-r-bar-fill{display:block;height:100%}
.oo-r-bar-fill.oo-r-pass{background:var(--oo-pass)}.oo-r-bar-fill.oo-r-fail{background:var(--oo-fail)}.oo-r-bar-fill.oo-r-flaky{background:var(--oo-flaky)}
.oo-r-bar-label{margin-left:8px}
.oo-r-muted,.oo-r-note{color:var(--oo-ink-3)}.oo-r-note{font-size:12px;margin:8px 0 0}
.oo-r-headline{font-size:16px;font-weight:600}
.oo-r-findings{list-style:none;padding:0;margin:0;display:flex;flex-direction:column;gap:10px}
.oo-r-finding{border-left:3px solid var(--oo-border-strong);padding:4px 0 4px 12px}
.oo-r-finding p{margin:4px 0}
.oo-r-evidence{margin:4px 0;color:var(--oo-ink-2);font-size:12px}
.oo-r-groups{margin:0 0 8px;padding-left:18px}
"""


# -- the command ----------------------------------------------------------------


def main(argv: Optional[Sequence[str]] = None, *, prog: str = "openodd-test-report") -> int:
    """``openodd-test-report``: coverage files as an HTML test report, with
    Claude's analysis (the Claude Agent SDK) unless ``--no-agent``."""
    parser = argparse.ArgumentParser(
        prog=prog,
        description="Turn *_coverage.json files into a standalone HTML test report: results, "
        "failed runs, ODD coverage, flakiness and the interactive views, with an analysis "
        "Claude writes through the Claude Agent SDK.",
    )
    parser.add_argument("paths", nargs="+", type=Path, help="coverage files, or directories to search")
    parser.add_argument("-o", "--output", type=Path, default=Path("openodd_test_report.html"),
                        help="the HTML page to write (default: openodd_test_report.html)")
    parser.add_argument("--title", default="ODD test report", help="the page's title")
    parser.add_argument("--no-agent", action="store_true", help="make no analysis: the figures only")
    parser.add_argument("--analysis", type=Path, metavar="JSON",
                        help="use this analysis (as --analysis-out wrote it) instead of asking Claude")
    parser.add_argument("--analysis-out", type=Path, metavar="JSON", help="also write the analysis here")
    parser.add_argument("--summary-json", type=Path, metavar="JSON", help="also write the report's figures here")
    parser.add_argument("--model", default=None, help="the Claude model (default: openodd.agent.DEFAULT_MODEL)")
    parser.add_argument("--language", default="English", help="the analysis's language (default: English)")
    parser.add_argument("--instructions", default="", help="more for Claude to consider, e.g. the pass criteria")
    parser.add_argument("--max-turns", type=int, default=None, help="the agent's turn limit")
    parser.add_argument("--max-budget-usd", type=float, default=None, help="the agent's spending limit")
    parser.add_argument("--no-views", action="store_true", help="leave out the interactive views")
    parser.add_argument("--cases-x", metavar="ATTRIBUTE", help="the case plot's horizontal attribute")
    parser.add_argument("--cases-y", metavar="ATTRIBUTE", help="the case plot's vertical attribute")
    args = parser.parse_args(argv)
    logging.basicConfig(level=logging.WARNING, format="%(levelname)s: %(message)s")

    report = TestReport.from_paths(args.paths, title=args.title)
    if not report.documents:
        print("No coverage files found.", file=sys.stderr)  # noqa: T201
        return 1
    analysis: Optional[ReportAnalysis] = None
    if args.analysis is not None:
        analysis = ReportAnalysis.from_dict(json.loads(args.analysis.read_text(encoding="utf-8")))
    elif not args.no_agent:
        from . import agent  # noqa: PLC0415 - the SDK is optional

        options: dict[str, Any] = {"language": args.language, "instructions": args.instructions}
        if args.model:
            options["model"] = args.model
        if args.max_turns is not None:
            options["max_turns"] = args.max_turns
        if args.max_budget_usd is not None:
            options["max_budget_usd"] = args.max_budget_usd
        try:
            analysis = agent.analyze(report, **options)
        except agent.AgentUnavailable as e:
            print(f"warning: no analysis: {e}", file=sys.stderr)  # noqa: T201
        except agent.AnalysisError as e:
            print(f"warning: the analysis failed: {e}", file=sys.stderr)  # noqa: T201
    if analysis is not None and args.analysis_out is not None:
        args.analysis_out.parent.mkdir(parents=True, exist_ok=True)
        args.analysis_out.write_text(json.dumps(analysis.to_dict(), indent=2, ensure_ascii=False),
                                     encoding="utf-8")
    if args.summary_json is not None:
        args.summary_json.parent.mkdir(parents=True, exist_ok=True)
        args.summary_json.write_text(json.dumps(report.summary(), indent=2, ensure_ascii=False),
                                     encoding="utf-8")
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(report.page(analysis=analysis, views=not args.no_views,
                                       cases_x=args.cases_x, cases_y=args.cases_y), encoding="utf-8")
    print(f"Wrote {args.output}", file=sys.stderr)  # noqa: T201
    return 0


if __name__ == "__main__":  # pragma: no cover
    sys.exit(main())
