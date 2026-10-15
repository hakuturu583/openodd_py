# Test report

`openodd-test-report` turns the coverage files of a campaign (`*_coverage.json`, one per
run) into one standalone HTML page a reviewer reads top to bottom, with an analysis
Claude writes through the [Claude Agent SDK](https://code.claude.com/docs/en/agent-sdk).

```bash
pip install "openodd-py[agent]"          # the SDK; without it the page has the figures only
openodd-test-report runs/ -o report.html --language Japanese
```

The page holds:

- **the verdict**: runs, passed, failed, pass rate, ODD coverage, flaky cells and cases;
- **the analysis** (Claude's): a verdict (`pass` / `fail` / `inconclusive`), a headline,
  a summary, findings by severity with the figures they rest on, and recommendations;
- **results** per scenario and environment, and the time spent inside / outside the ODD;
- **every failed run**: its file, case, when and in which ODD cell it failed;
- **ODD coverage** per item, with its holes;
- **flakiness**: the flaky and always-failing ODD cells, and the flaky cases;
- **the interactive views**: the ODD matrix ([coverage_view.md](coverage_view.md)), the
  drawn cases ([case_plot.md](case_plot.md)) and the scenario's realisation ([drift.md](drift.md)).

Every number on the page is the merged report's own; the analysis only reads them.

## How Claude reads the report

`openodd.agent.analyze` runs the Claude Agent SDK with **no built-in tools** -- no shell,
no files -- and serves it, in-process, tools that read this report only:

| Tool | Returns |
|---|---|
| `summary` | `TestReport.summary()`: the counts, results per scenario / environment, coverage, failed runs, flaky and failing cells, flaky cases |
| `markdown_report` | the merged coverage report, as `openodd-coverage` prints it |
| `failed_runs` | the failed runs, by scenario / environment |
| `run` | one run's case, outcome and ODD exposure |
| `coverage_item` | one cover item's buckets |
| `odd_matrix` | `CoverageView.matrix` of any two attributes, drilled into any cells |
| `drift` | `CasePlot.drift`: drawn against measured, and the knob calibration |

Claude answers in the `openodd.agent.ANALYSIS_SCHEMA` JSON schema (the SDK's structured
output), which becomes a `ReportAnalysis`. Claude is reached as the SDK reaches it:
`ANTHROPIC_API_KEY`, or a logged-in Claude Code. The model defaults to
`openodd.agent.DEFAULT_MODEL`.

## Options

| Option | |
|---|---|
| `-o FILE` | the page (default `openodd_test_report.html`) |
| `--no-agent` | no analysis: the figures only |
| `--language` | the analysis's language (default English) |
| `--instructions` | more for Claude to weigh, e.g. `"the release bar is a 95% pass rate"` |
| `--model`, `--max-turns`, `--max-budget-usd` | the agent's model and limits |
| `--analysis-out JSON` / `--analysis JSON` | keep an analysis, and build a page from a kept one without asking Claude again |
| `--summary-json JSON` | also write the figures |
| `--no-views`, `--cases-x`, `--cases-y` | leave out the interactive views; the case plot's axes |

If the SDK or Claude Code is missing, or the analysis fails, the command warns and still
writes the page.

## From Python

```python
import openodd

report = openodd.TestReport.from_paths(["runs/"], title="Nightly cut-in")
analysis = openodd.agent.analyze(report, language="Japanese", max_budget_usd=2.0)
Path("report.html").write_text(report.page(analysis=analysis), encoding="utf-8")
```

`await openodd.agent.analyze_async(...)` inside an event loop; `options={...}` passes more
`ClaudeAgentOptions` fields. `report.html(analysis=...)` is the fragment, for a page of
your own (with `openodd.view.styles()` and `openodd.view.script()`).
