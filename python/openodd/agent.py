"""Claude reads a test report: the Claude Agent SDK over the merged coverage.

:func:`analyze` hands a :class:`~.testreport.TestReport` to Claude through the
`Claude Agent SDK <https://code.claude.com/docs/en/agent-sdk>`_
(``pip install "openodd-py[agent]"``). Claude gets no shell and no files --
only tools, served in-process, that read this report:

* ``summary`` -- the report's figures (:meth:`~.testreport.TestReport.summary`);
* ``markdown_report`` -- the merged coverage report, as ``openodd-coverage`` prints it;
* ``failed_runs`` / ``run`` -- the failed runs, and one run's case, outcome and ODD exposure;
* ``coverage_item`` -- one cover item's buckets: hits, runs, seconds, holes;
* ``odd_matrix`` -- the ODD matrix of any two attributes, drilled into any cells;
* ``drift`` -- how far the scenario staged each run from what the sampler drew.

How to analyse is an Agent Skill shipped with the package, ``odd-test-analysis``
in the ``openodd`` Claude Code plugin (:data:`PLUGIN_DIR`): the workflow, what
every figure means and the triage rules. Claude loads it with the ``Skill``
tool and may ``Read`` its reference files -- and no other file. The prompts are
files of the plugin too (``prompts/system.md``, ``prompts/task.md``); pass your
own (``system_prompt=``, ``task_prompt=``), and more plugins with skills of your
own -- house pass criteria, say -- with ``plugins=`` and ``skills=``.

Claude answers with a :class:`~.testreport.ReportAnalysis` -- a verdict, a
headline, a summary, findings with the figures they rest on, and
recommendations -- that :meth:`~.testreport.TestReport.page` puts at the top
of the report::

    report = openodd.TestReport.from_paths(["runs/"])
    analysis = openodd.agent.analyze(report, language="Japanese")
    Path("report.html").write_text(report.page(analysis=analysis))

Claude is reached as the SDK reaches it: ``ANTHROPIC_API_KEY``, or a logged-in
Claude Code.
"""

from __future__ import annotations

import asyncio
import json
import logging
from collections.abc import Mapping, Sequence
from pathlib import Path
from typing import TYPE_CHECKING, Any, Optional

from .testreport import SEVERITIES, VERDICTS, ReportAnalysis
from .view import CasePlot, CoverageView

if TYPE_CHECKING:
    from .testreport import TestReport

__all__ = [
    "ANALYSIS_SCHEMA",
    "DEFAULT_MODEL",
    "PLUGIN_DIR",
    "SERVER_NAME",
    "SKILL_DIR",
    "SKILL_NAME",
    "AgentUnavailable",
    "AnalysisError",
    "analyze",
    "analyze_async",
    "default_system_prompt",
    "default_task_prompt",
    "report_tools",
]

logger = logging.getLogger(__name__)

#: The model asked when none is named.
DEFAULT_MODEL = "claude-opus-5-5"
#: The name the report's tools are served under (``mcp__openodd__summary``, ...).
SERVER_NAME = "openodd"

#: What Claude answers with: a :class:`~.testreport.ReportAnalysis`.
ANALYSIS_SCHEMA: dict[str, Any] = {
    "type": "object",
    "additionalProperties": False,
    "required": ["verdict", "headline", "summary", "findings", "recommendations"],
    "properties": {
        "verdict": {"type": "string", "enum": list(VERDICTS)},
        "headline": {"type": "string", "description": "One sentence: the campaign's outcome."},
        "summary": {"type": "string", "description": "A paragraph for a reviewer."},
        "findings": {
            "type": "array",
            "items": {
                "type": "object",
                "additionalProperties": False,
                "required": ["severity", "title", "detail", "evidence"],
                "properties": {
                    "severity": {"type": "string", "enum": list(SEVERITIES)},
                    "title": {"type": "string"},
                    "detail": {"type": "string"},
                    "evidence": {"type": "array", "items": {"type": "string"},
                                 "description": "Figures from the tools' results, quoted."},
                },
            },
        },
        "recommendations": {"type": "array", "items": {"type": "string"}},
    },
}

#: The ``openodd`` Claude Code plugin shipped with the package: the
#: ``odd-test-analysis`` skill and the analysis prompts.
PLUGIN_DIR = Path(__file__).resolve().parent / "claude_plugin"
#: The analysis skill, as the agent names it.
SKILL_NAME = "openodd:odd-test-analysis"
#: The skill's own directory (``SKILL.md`` and ``references/``).
SKILL_DIR = PLUGIN_DIR / "skills" / "odd-test-analysis"


def default_system_prompt() -> str:
    """The analyst's system prompt (``prompts/system.md`` of the plugin)."""
    return (PLUGIN_DIR / "prompts" / "system.md").read_text(encoding="utf-8").strip()


def default_task_prompt() -> str:
    """The task prompt's template (``prompts/task.md`` of the plugin): ``{title}``,
    ``{runs}``, ``{passed}``, ``{failed}``, ``{no_outcome}``, ``{language}`` and
    ``{instructions}`` are filled in."""
    return (PLUGIN_DIR / "prompts" / "task.md").read_text(encoding="utf-8").strip()


class AgentUnavailable(RuntimeError):
    """The Claude Agent SDK, or the Claude Code it drives, is not installed."""


class AnalysisError(RuntimeError):
    """Claude did not return an analysis."""


def _rounded(value: Any) -> Any:
    """*value* with its floats to 4 significant digits: figures Claude quotes as given."""
    if isinstance(value, float):
        return float(f"{value:.4g}")
    if isinstance(value, Mapping):
        return {k: _rounded(v) for k, v in value.items()}
    if isinstance(value, (list, tuple)):
        return [_rounded(v) for v in value]
    return value


def _text(value: Any) -> dict[str, Any]:
    return {"content": [{"type": "text", "text": json.dumps(_rounded(value), ensure_ascii=False, default=str)}]}


def _error(message: str) -> dict[str, Any]:
    return {"content": [{"type": "text", "text": message}], "is_error": True}


def _conditions(raw: Any) -> list[tuple[str, str]]:
    if isinstance(raw, Mapping):
        return [(str(k), str(v)) for k, v in raw.items()]
    return [(str(a), str(b)) for a, b in (raw or [])]


def report_tools(report: "TestReport") -> list[Any]:
    """The tools Claude reads *report* with, as Claude Agent SDK tools."""
    sdk = _sdk()
    tool = sdk.tool

    @tool("summary", "The report's figures: runs passed / failed per scenario and environment, "
          "ODD exposure, coverage grade and holes per item, every failed run, the flaky and "
          "always-failing ODD cells, and the flaky cases.", {"type": "object", "properties": {}})
    async def summary(args: dict[str, Any]) -> dict[str, Any]:
        return _text(report.summary())

    @tool("markdown_report", "The merged coverage report as Markdown: per item, every bucket's "
          "hits, runs, seconds and metres, and the cases table.",
          {"type": "object", "properties": {"max_holes": {"type": "integer", "minimum": 1}}})
    async def markdown_report(args: dict[str, Any]) -> dict[str, Any]:
        return {"content": [{"type": "text", "text": report.markdown(max_holes=int(args.get("max_holes", 20)))}]}

    @tool("failed_runs", "The failed runs: their case values, when and in which ODD cell they failed.",
          {"type": "object", "properties": {"scenario": {"type": "string"},
                                            "environment": {"type": "string"},
                                            "limit": {"type": "integer", "minimum": 1}}})
    async def failed_runs(args: dict[str, Any]) -> dict[str, Any]:
        out = [f for f in report.summary()["failures"]
               if args.get("scenario") in (None, f["scenario"])
               and args.get("environment") in (None, f["environment"])]
        return _text(out[: int(args.get("limit", 200))])

    @tool("run", "One run (by its document index): scenario, case, outcome, and its ODD exposure.",
          {"type": "object", "required": ["document"],
           "properties": {"document": {"type": "integer", "minimum": 0}}})
    async def run(args: dict[str, Any]) -> dict[str, Any]:
        i = int(args["document"])
        if not 0 <= i < len(report.documents):
            return _error(f"no document {i}: there are {len(report.documents)}")
        doc = report.documents[i]
        odd = dict(doc.get("odd") or {})
        odd.pop("cells", None)  # per-step tables: the matrix tool reads them
        return _text({"document": i, "name": report.names[i], "scenario": doc.get("scenario"),
                      "case": doc.get("case"), "outcome": doc.get("outcome"), "odd": odd})

    @tool("coverage_item", "One cover item: its buckets with hits, runs and exposure, which are "
          "covered, the holes, and the buckets outside the ODD.",
          {"type": "object", "required": ["name"], "properties": {"name": {"type": "string"}}})
    async def coverage_item(args: dict[str, Any]) -> dict[str, Any]:
        entry = report.report.entry(str(args["name"]))
        if entry is None:
            names = [e.name for e in report.report.entries]
            return _error(f"no item {args['name']!r}; items: {', '.join(names)}")
        return _text(json.loads(entry.to_json()))

    @tool("odd_matrix", "The ODD matrix of two attributes: per pair of buckets, the runs that were "
          "there, how many failed and failed there, and the cell's state (flaky, failing, passing, "
          "outside the ODD, not sampled). `conditions` drills into cells: [[attribute, bucket], ...].",
          {"type": "object", "properties": {
              "x": {"type": "string"}, "y": {"type": "string"}, "scenario": {"type": "string"},
              "conditions": {"type": "array", "items": {"type": "array", "items": {"type": "string"}}}}})
    async def odd_matrix(args: dict[str, Any]) -> dict[str, Any]:
        view = CoverageView(report.documents)
        if not view.runs:
            return _error("no run recorded the ODD's cells")
        unknown = [a for a in (args.get("x"), args.get("y"), *(c[0] for c in _conditions(args.get("conditions"))))
                   if a and a not in view.attributes]
        if unknown:  # the view would quietly pick others
            return _error(f"no attribute {', '.join(unknown)}; attributes: {', '.join(view.attributes)}")
        try:
            return _text(view.matrix(x=args.get("x"), y=args.get("y"), scenario=args.get("scenario"),
                                     conditions=_conditions(args.get("conditions"))))
        except Exception as e:  # noqa: BLE001 - an unknown attribute, say: Claude can retry
            return _error(f"{e}; attributes: {', '.join(view.attributes)}")

    @tool("drift", "How far the scenario staged each run from what the sampler drew, per "
          "attribute (worst first): bias, MAE, bucket hit rate and the knob calibration a fit suggests.",
          {"type": "object", "properties": {"scenario": {"type": "string"}}})
    async def drift(args: dict[str, Any]) -> dict[str, Any]:
        plot = CasePlot(report.documents)
        if not plot.cases:
            return _error("no run recorded a case and an outcome")
        return _text(plot.drift(scenario=args.get("scenario")))

    return [summary, markdown_report, failed_runs, run, coverage_item, odd_matrix, drift]


def _sdk() -> Any:
    try:
        import claude_agent_sdk  # noqa: PLC0415 - optional
    except ImportError as e:
        raise AgentUnavailable(
            'the Claude Agent SDK is not installed: pip install "openodd-py[agent]"') from e
    return claude_agent_sdk


def _prompt(report: "TestReport", template: str, language: str, instructions: str) -> str:
    s = report.summary()
    fields = {k: s[k] for k in ("title", "runs", "passed", "failed", "no_outcome")}
    fields["language"] = language
    fields["instructions"] = f"Also consider: {instructions}" if instructions else ""
    for key, value in fields.items():  # only these: other braces stay as written
        template = template.replace("{" + key + "}", str(value))
    return template.strip()


def _read_only_under(roots: Sequence[Path]) -> Any:
    """A PreToolUse hook: ``Read`` only files under *roots* (the skills')."""
    resolved = [r.resolve() for r in roots]

    async def hook(input_data: Mapping[str, Any], tool_use_id: Optional[str], context: Any) -> dict[str, Any]:
        path = (input_data.get("tool_input") or {}).get("file_path") or ""
        base = input_data.get("cwd")  # Claude Code's own, which may not be ours
        try:
            target = (Path(base) / path if base else Path(path)).resolve() if path else None
        except (OSError, ValueError):
            target = None
        if target is not None and target.is_absolute() and any(target.is_relative_to(r) for r in resolved):
            return {}
        return {"hookSpecificOutput": {
            "hookEventName": "PreToolUse", "permissionDecision": "deny",
            "permissionDecisionReason": "only the skills' files may be read; "
                                        "the report is read with the mcp__openodd__ tools"}}

    return hook


def _parse(result: Any) -> dict[str, Any]:
    """The analysis in a result message: its structured output, or JSON in its text."""
    if isinstance(getattr(result, "structured_output", None), Mapping):
        return dict(result.structured_output)
    text = getattr(result, "result", None) or ""
    start, end = text.find("{"), text.rfind("}")
    if start >= 0 and end > start:
        try:
            raw = json.loads(text[start:end + 1])
        except ValueError:
            pass
        else:
            if isinstance(raw, Mapping):
                return dict(raw)
    raise AnalysisError(f"no analysis in Claude's answer: {text[:200]!r}")


async def analyze_async(
    report: "TestReport",
    *,
    model: str = DEFAULT_MODEL,
    language: str = "English",
    instructions: str = "",
    effort: Optional[str] = "high",
    max_turns: int = 30,
    max_budget_usd: Optional[float] = None,
    system_prompt: Optional[str] = None,
    task_prompt: Optional[str] = None,
    skill: bool = True,
    plugins: Sequence[Path] = (),
    skills: Sequence[str] = (),
    options: Optional[Mapping[str, Any]] = None,
) -> ReportAnalysis:
    """:func:`analyze`, for a caller already in an event loop."""
    sdk = _sdk()
    tools = report_tools(report)
    server = sdk.create_sdk_mcp_server(name=SERVER_NAME, version="1.0.0", tools=tools)
    kwargs: dict[str, Any] = {
        "system_prompt": default_system_prompt() if system_prompt is None else system_prompt,
        "model": model,
        "mcp_servers": {SERVER_NAME: server},
        "tools": [],  # no built-in tools: no shell, no files
        "allowed_tools": [f"mcp__{SERVER_NAME}__{t.name}" for t in tools],
        "setting_sources": [],  # nothing from the user's or the project's settings
        "max_turns": max_turns,
        "output_format": {"type": "json_schema", "schema": ANALYSIS_SCHEMA},
    }
    enabled = ([SKILL_NAME] if skill else []) + list(skills)
    if plugins and not skills:
        logger.warning("agent: plugins %s loaded, but none of their skills enabled (skills=)",
                       ", ".join(map(str, plugins)))
    if enabled:
        dirs = ([PLUGIN_DIR] if skill else []) + [Path(p) for p in plugins]
        kwargs["plugins"] = [{"type": "local", "path": str(d)} for d in dirs]
        kwargs["skills"] = enabled
        # The Skill tool, and Read for the skills' reference files only.
        kwargs["tools"] = ["Skill", "Read"]
        kwargs["allowed_tools"].append("Read")
        kwargs["hooks"] = {"PreToolUse": [sdk.HookMatcher(matcher="Read", hooks=[_read_only_under(dirs)])]}
    if effort is not None:
        kwargs["effort"] = effort
    if max_budget_usd is not None:
        kwargs["max_budget_usd"] = max_budget_usd
    kwargs.update(options or {})
    prompt = _prompt(report, default_task_prompt() if task_prompt is None else task_prompt,
                     language, instructions)
    result = None
    served_by = model
    try:
        async for message in sdk.query(prompt=prompt,
                                       options=sdk.ClaudeAgentOptions(**kwargs)):
            if isinstance(message, sdk.AssistantMessage) and message.model:
                served_by = message.model
            elif isinstance(message, sdk.ResultMessage):
                result = message
    except sdk.CLINotFoundError as e:
        raise AgentUnavailable(f"Claude Code was not found: {e}") from e
    except Exception as e:  # noqa: BLE001 - the SDK raises bare errors too (timeouts, bad options)
        raise AnalysisError(f"{type(e).__name__}: {e}") from e
    if result is None:
        raise AnalysisError("Claude returned no result")
    if result.is_error:
        raise AnalysisError(f"{result.subtype}: {'; '.join(result.errors or []) or result.result or ''}")
    raw = _parse(result)
    raw.setdefault("author", served_by)
    raw.setdefault("cost_usd", result.total_cost_usd)
    return ReportAnalysis.from_dict(raw)


def analyze(report: "TestReport", **kwargs: Any) -> ReportAnalysis:
    """Have Claude analyse *report* through the Claude Agent SDK.

    Args:
        report: The test report.
        model: The Claude model (default :data:`DEFAULT_MODEL`).
        language: The language to write the analysis in.
        instructions: More for Claude to consider -- the pass criteria, say.
        system_prompt: The system prompt (default :func:`default_system_prompt`).
        task_prompt: The task prompt's template (default :func:`default_task_prompt`).
        skill: Whether Claude has the ``odd-test-analysis`` skill.
        plugins: More Claude Code plugin directories, with skills of your own.
        skills: Their skills to enable (``plugin:skill``).
        effort: The effort Claude spends (``low`` ... ``max``; ``None``: the model's).
        max_turns: The most turns the agent takes.
        max_budget_usd: The most the analysis may cost.
        options: More :class:`claude_agent_sdk.ClaudeAgentOptions` fields,
            which override these.

    Raises:
        AgentUnavailable: The SDK or Claude Code is not installed.
        AnalysisError: Claude returned no analysis.
    """
    try:
        asyncio.get_running_loop()
    except RuntimeError:
        return asyncio.run(analyze_async(report, **kwargs))
    raise RuntimeError("analyze() was called from a running event loop: await analyze_async()")
