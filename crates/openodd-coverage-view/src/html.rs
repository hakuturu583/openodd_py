//! The matrix as HTML: a component to embed in another tool's pages.
//!
//! Everything is a string: no template engine, no framework, no network.
//! A host puts [`styles`] and [`script`] on its page once and as many
//! fragments as it likes; [`page`] wraps them into a standalone document.
//! Every class is prefixed `oo-` and scoped under `.oo-cov`; the colours are
//! CSS variables (`--oo-pass`, `--oo-fail`, `--oo-flaky`, `--oo-outside`,
//! `--oo-unsampled`, ...) a host overrides to match its theme.
//!
//! Clicking into a cell works one of two ways:
//!
//! * **Host-driven** ([`fragment`]): each cell carries the conditions it
//!   stands for (`data-oo-drill`, a JSON list of `[attribute, bucket]`), and
//!   a click dispatches a bubbling, cancelable `openodd:drill` event with
//!   them (`event.detail.conditions`). With [`Options::link`], a cell is also
//!   a link (`?at=attribute%3Dbucket&...`), so a host that re-renders on the
//!   server -- plain links, htmx -- needs no script at all.
//! * **Self-contained** ([`explorer`]): the levels below are rendered ahead
//!   of time, as hidden panels, and a click shows the panel of that cell.
//!   That is what a static report needs.
//!
//! Either way, every number is computed in Rust ([`crate::matrix`]); the
//! script only shows, hides and dispatches.

use std::fmt::Write as _;

use crate::data::{Dataset, ABOVE_RANGE, BELOW_RANGE, MISSING};
use crate::matrix::{Cell, CellState, Matrix, Query};

/// The words the component shows, for a host to translate.
#[derive(Debug, Clone, PartialEq)]
pub struct Labels {
    pub title: String,
    pub whole: String,
    pub runs: String,
    pub failed: String,
    pub failed_here: String,
    pub fail_rate: String,
    pub interval: String,
    pub seconds: String,
    pub no_runs: String,
    pub tentative: String,
    pub state_flaky: String,
    pub state_fail: String,
    pub state_pass: String,
    pub state_unknown: String,
    pub state_unsampled: String,
    pub state_outside: String,
    pub below: String,
    pub above: String,
    pub missing: String,
    pub runs_without_outcome: String,
    pub pooled_environments: String,
    pub no_attributes: String,
    pub unsampled_documents: String,
    pub click_hint: String,
    pub coverage: String,
    pub cells: String,
    pub flaky_cells: String,
    pub failing_cells: String,
    pub reached: String,
    pub here: String,
    pub time: String,
    pub of_cells: String,
    pub truncated: String,
    pub levels: String,
}

impl Labels {
    /// Set one label by its field name; `false` if there is no such label.
    pub fn set(&mut self, name: &str, value: &str) -> bool {
        let slot = match name {
            "title" => &mut self.title,
            "whole" => &mut self.whole,
            "runs" => &mut self.runs,
            "failed" => &mut self.failed,
            "failed_here" => &mut self.failed_here,
            "fail_rate" => &mut self.fail_rate,
            "interval" => &mut self.interval,
            "seconds" => &mut self.seconds,
            "no_runs" => &mut self.no_runs,
            "tentative" => &mut self.tentative,
            "state_flaky" => &mut self.state_flaky,
            "state_fail" => &mut self.state_fail,
            "state_pass" => &mut self.state_pass,
            "state_unknown" => &mut self.state_unknown,
            "state_unsampled" => &mut self.state_unsampled,
            "state_outside" => &mut self.state_outside,
            "below" => &mut self.below,
            "above" => &mut self.above,
            "missing" => &mut self.missing,
            "runs_without_outcome" => &mut self.runs_without_outcome,
            "pooled_environments" => &mut self.pooled_environments,
            "no_attributes" => &mut self.no_attributes,
            "unsampled_documents" => &mut self.unsampled_documents,
            "click_hint" => &mut self.click_hint,
            "coverage" => &mut self.coverage,
            "cells" => &mut self.cells,
            "flaky_cells" => &mut self.flaky_cells,
            "failing_cells" => &mut self.failing_cells,
            "reached" => &mut self.reached,
            "here" => &mut self.here,
            "time" => &mut self.time,
            "of_cells" => &mut self.of_cells,
            "truncated" => &mut self.truncated,
            "levels" => &mut self.levels,
            _ => return false,
        };
        *slot = value.to_string();
        true
    }
}

impl Default for Labels {
    fn default() -> Self {
        let s = |x: &str| x.to_string();
        Labels {
            title: s("ODD coverage"),
            whole: s("Whole ODD"),
            runs: s("Runs"),
            failed: s("Failed"),
            failed_here: s("Failed here"),
            fail_rate: s("Fail rate"),
            interval: s("95% interval"),
            seconds: s("s"),
            no_runs: s("none"),
            tentative: s("Too few runs to tell"),
            state_flaky: s("Flaky: both outcomes"),
            state_fail: s("Fails"),
            state_pass: s("Passes"),
            state_unknown: s("Covered, no outcome"),
            state_unsampled: s("Not sampled"),
            state_outside: s("Outside the ODD"),
            below: s("below range"),
            above: s("above range"),
            missing: s("no value"),
            runs_without_outcome: s("Runs without an outcome"),
            pooled_environments: s(
                "Runs of different environments are pooled here; select one to tell flakiness from regressions.",
            ),
            no_attributes: s("No ODD attribute with cells to lay out."),
            unsampled_documents: s("coverage files without ODD cells are not shown"),
            click_hint: s("Click a cell to look inside it"),
            coverage: s("ODD covered"),
            cells: s("cells"),
            flaky_cells: s("Flaky cells"),
            failing_cells: s("Failing cells"),
            reached: s("reached"),
            here: s("here"),
            time: s("Time in it"),
            of_cells: s("of {n} cells"),
            truncated: s(
                "Some cells were not rendered ahead of time (the page holds at most {n} levels); \
                 render fewer levels below, or start from a cell.",
            ),
            levels: s("Drill-down levels"),
        }
    }
}

/// How a fragment is rendered.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Options {
    /// The id of the component's root element (and the prefix of its panels'
    /// ids); `oo-cov` when empty.
    pub id: String,
    /// A URL template that makes each cell (and breadcrumb) a link:
    /// `{conditions}` is replaced by `at=attribute%3Dbucket&at=...`.
    pub link: Option<String>,
    pub labels: Labels,
}

impl Options {
    fn root_id(&self) -> &str {
        if self.id.is_empty() {
            "oo-cov"
        } else {
            &self.id
        }
    }
}

/// HTML-escape text (and attribute values).
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// Percent-encode a query value.
fn encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => {
                let _ = write!(out, "%{b:02X}");
            }
        }
    }
    out
}

/// `at=attribute%3Dbucket&...`: the conditions as a query string.
pub fn conditions_query(conditions: &[(String, String)]) -> String {
    conditions
        .iter()
        .map(|(a, l)| format!("at={}", encode(&format!("{a}={l}"))))
        .collect::<Vec<_>>()
        .join("&")
}

fn conditions_json(conditions: &[(String, String)]) -> String {
    serde_json::to_string(
        &conditions
            .iter()
            .map(|(a, l)| [a.as_str(), l.as_str()])
            .collect::<Vec<_>>(),
    )
    .expect("JSON")
}

fn label_title(label: &str, labels: &Labels) -> String {
    match label {
        MISSING => labels.missing.clone(),
        BELOW_RANGE => labels.below.clone(),
        ABOVE_RANGE => labels.above.clone(),
        l => l.to_string(),
    }
}

fn pct(x: f64) -> String {
    format!("{:.0}%", x * 100.0)
}

fn state_label(state: CellState, labels: &Labels) -> &str {
    match state {
        CellState::Flaky => &labels.state_flaky,
        CellState::Fail => &labels.state_fail,
        CellState::Pass => &labels.state_pass,
        CellState::Unknown => &labels.state_unknown,
        CellState::Unsampled => &labels.state_unsampled,
        CellState::Outside => &labels.state_outside,
    }
}

/// A cell's tooltip: a plain-text summary (its accessible name) and the
/// rows the script shows in its tooltip card (`data-oo-tip`).
fn cell_tip(c: &Cell, place: &str, labels: &Labels) -> (String, String) {
    let mut state = state_label(c.state, labels).to_string();
    if c.outside && c.visited() {
        let _ = write!(state, " ({})", labels.reached);
    }
    let mut rows: Vec<(String, String)> = Vec::new();
    if c.runs > 0 {
        let (lo, hi) = c.interval().unwrap_or((0.0, 1.0));
        rows.push((labels.runs.clone(), c.runs.to_string()));
        rows.push((labels.failed.clone(), c.failed.to_string()));
        rows.push((labels.failed_here.clone(), c.fail_at_cell.to_string()));
        rows.push((labels.fail_rate.clone(), pct(c.fail_rate().unwrap_or(0.0))));
        rows.push((
            labels.interval.clone(),
            format!("{} \u{2013} {}", pct(lo), pct(hi)),
        ));
    } else if !c.visited() {
        rows.push((labels.runs.clone(), labels.no_runs.clone()));
    }
    if c.runs_unknown > 0 {
        rows.push((
            labels.runs_without_outcome.clone(),
            c.runs_unknown.to_string(),
        ));
    }
    if c.seconds > 0.0 {
        rows.push((
            labels.time.clone(),
            format!("{:.1} {}", c.seconds, labels.seconds),
        ));
    }
    let mut text = format!("{place}: {state}");
    for (k, v) in &rows {
        let _ = write!(text, ", {k} {v}");
    }
    if c.tentative {
        let _ = write!(text, " ({})", labels.tentative);
    }
    let tip = serde_json::json!({
        "place": place,
        "state": state,
        "kind": c.state.as_str(),
        "tentative": if c.tentative { Some(&labels.tentative) } else { None },
        "rows": rows,
    });
    (text, tip.to_string())
}

/// The icon of a state: what says it besides the colour.
fn state_icon(state: CellState) -> &'static str {
    match state {
        CellState::Pass => "\u{2713}",
        CellState::Fail => "\u{2715}",
        CellState::Flaky => "\u{223F}",
        CellState::Unknown => "?",
        CellState::Outside => "\u{2298}",
        CellState::Unsampled => "",
    }
}

/// What a click on a cell (or crumb) does: its attributes.
enum Target<'a> {
    /// Nothing: no run there, or nothing left to lay out inside it.
    None,
    /// Host-driven: dispatch (and follow the link, if any).
    Drill(&'a [(String, String)]),
    /// Self-contained: show this panel.
    Open(String),
}

fn target_attrs(target: &Target, opts: &Options) -> (String, &'static str) {
    match target {
        Target::None => (" disabled".into(), "button"),
        Target::Drill(conds) => {
            let mut a = format!(" data-oo-drill=\"{}\"", escape(&conditions_json(conds)));
            if let Some(link) = &opts.link {
                let href = link.replace("{conditions}", &conditions_query(conds));
                let _ = write!(a, " href=\"{}\"", escape(&href));
                return (a, "a");
            }
            (a, "button")
        }
        Target::Open(id) => (
            format!(" data-oo-open=\"{}\" href=\"#{}\"", escape(id), escape(id)),
            "a",
        ),
    }
}

fn cell_html(out: &mut String, c: &Cell, place: &str, target: Target, opts: &Options) {
    let labels = &opts.labels;
    let (attrs, tag) = target_attrs(&target, opts);
    let mut class = format!("oo-cell oo-{}", c.state.as_str());
    if c.tentative {
        class += " oo-tentative";
    }
    let tag_attrs = if tag == "button" {
        " type=\"button\""
    } else {
        ""
    };
    let (text, tip) = cell_tip(c, place, labels);
    let _ = write!(
        out,
        "<{tag}{tag_attrs} class=\"{class}\" aria-label=\"{}\" data-oo-tip=\"{}\"{attrs}>",
        escape(&text),
        escape(&tip)
    );
    let icon = state_icon(c.state);
    if !icon.is_empty() {
        let _ = write!(
            out,
            "<span class=\"oo-ic\" aria-hidden=\"true\">{icon}</span>"
        );
    }
    match c.fail_rate() {
        Some(rate) => {
            let _ = write!(
                out,
                "<span class=\"oo-rate\">{}</span><span class=\"oo-n\">{} / {}</span>",
                pct(rate),
                c.failed,
                c.runs
            );
            if c.fail_at_cell > 0 {
                let _ = write!(
                    out,
                    "<span class=\"oo-badge\" aria-hidden=\"true\">{}</span>",
                    c.fail_at_cell
                );
            }
        }
        None if c.runs_unknown > 0 => {
            let _ = write!(
                out,
                "<span class=\"oo-rate\">&ndash;</span><span class=\"oo-n\">{}</span>",
                c.runs_unknown
            );
        }
        None => {}
    }
    let _ = write!(out, "</{tag}>");
}

/// How each cell of `m` can be clicked into.
type Targets<'a> = dyn Fn(usize, usize) -> Option<String> + 'a;
/// Whether there is anything left to lay out inside these conditions.
type HasDepth<'a> = dyn Fn(&[(String, String)]) -> bool + 'a;

/// One matrix as a panel: breadcrumbs, legend, grid. `open` gives, in the
/// self-contained explorer, the panel of a cell (row, column); `crumbs` the
/// panels of the levels above, outermost first, each with how many of the
/// conditions it was inside.
#[allow(clippy::too_many_arguments)]
fn panel(
    truncated: bool,
    out: &mut String,
    m: &Matrix,
    opts: &Options,
    panel_id: &str,
    hidden: bool,
    open: Option<&Targets>,
    crumbs: Option<&[(String, usize)]>,
    has_depth: &HasDepth,
) {
    let labels = &opts.labels;
    let _ = write!(
        out,
        "<section class=\"oo-panel\" id=\"{}\" tabindex=\"-1\"{}>",
        escape(panel_id),
        if hidden { " hidden" } else { "" }
    );
    // Breadcrumbs: the whole ODD, then each level clicked into.
    let _ = write!(
        out,
        "<nav class=\"oo-crumbs\" aria-label=\"{}\">",
        escape(&labels.levels)
    );
    let conds = &m.query.conditions;
    let mut levels: Vec<(usize, Option<String>)> = match crumbs {
        Some(up) => up.iter().map(|(id, n)| (*n, Some(id.clone()))).collect(),
        None => (0..conds.len()).map(|n| (n, None)).collect(),
    };
    levels.push((conds.len(), None));
    let mut before = 0usize;
    for (j, (n, id)) in levels.iter().enumerate() {
        let text = if *n == 0 {
            escape(&labels.whole)
        } else {
            conds[before.min(*n)..*n]
                .iter()
                .map(|(a, l)| {
                    format!(
                        "<code>{}</code> = {}",
                        escape(a),
                        escape(&label_title(l, labels))
                    )
                })
                .collect::<Vec<_>>()
                .join(", ")
        };
        before = *n;
        if j > 0 {
            out.push_str("<span class=\"oo-sep\">&rsaquo;</span>");
        }
        if j + 1 == levels.len() {
            let _ = write!(
                out,
                "<span class=\"oo-at\" aria-current=\"location\">{text}</span>"
            );
            continue;
        }
        let target = match id {
            Some(id) => Target::Open(id.clone()),
            None => Target::Drill(&conds[..*n]),
        };
        let (attrs, tag) = target_attrs(&target, opts);
        let tag_attrs = if tag == "button" {
            " type=\"button\""
        } else {
            ""
        };
        let _ = write!(
            out,
            "<{tag}{tag_attrs} class=\"oo-crumb\"{attrs}>{text}</{tag}>"
        );
    }
    out.push_str("</nav>");
    let t = &m.total;
    let counts = m.counts();
    let inside: usize = m.cells.iter().flatten().filter(|c| !c.outside).count();
    let covered: usize = m
        .cells
        .iter()
        .flatten()
        .filter(|c| !c.outside && c.visited())
        .count();
    let stat = |out: &mut String, key: &str, value: String, sub: String, kind: &str| {
        let _ = write!(
            out,
            "<div class=\"oo-stat oo-stat-{kind}\"><span class=\"oo-k\">{}</span>\
             <span class=\"oo-v\">{value}</span><span class=\"oo-s\">{sub}</span></div>",
            escape(key)
        );
    };
    out.push_str("<div class=\"oo-stats\">");
    stat(
        out,
        &labels.runs,
        (t.runs + t.runs_unknown).to_string(),
        format!("{} {}", t.failed, escape(&labels.failed.to_lowercase())),
        "runs",
    );
    stat(
        out,
        &labels.coverage,
        if inside == 0 {
            "&ndash;".into()
        } else {
            pct(covered as f64 / inside as f64)
        },
        format!("{covered} / {inside} {}", escape(&labels.cells)),
        "coverage",
    );
    let of_cells = escape(&labels.of_cells.replace("{n}", &covered.to_string()));
    stat(
        out,
        &labels.flaky_cells,
        counts["flaky"].to_string(),
        of_cells.clone(),
        "flaky",
    );
    stat(
        out,
        &labels.failing_cells,
        counts["fail"].to_string(),
        of_cells,
        "fail",
    );
    out.push_str("</div>");
    if m.environments.len() > 1 {
        let _ = write!(
            out,
            "<p class=\"oo-warn\">{}</p>",
            escape(&labels.pooled_environments)
        );
    }
    // How the cells split by state, then the legend with each count.
    let states = [
        CellState::Flaky,
        CellState::Fail,
        CellState::Pass,
        CellState::Unknown,
        CellState::Unsampled,
        CellState::Outside,
    ];
    let all: usize = counts.values().sum();
    if all > 0 {
        out.push_str("<div class=\"oo-bar\" aria-hidden=\"true\">");
        for state in states {
            let n = counts[state.as_str()];
            if n > 0 {
                let _ = write!(
                    out,
                    "<span class=\"oo-{}\" style=\"flex-grow:{n}\"></span>",
                    state.as_str()
                );
            }
        }
        out.push_str("</div>");
    }
    out.push_str("<div class=\"oo-legend\">");
    for state in states {
        let n = counts[state.as_str()];
        if n == 0 && matches!(state, CellState::Unknown) {
            continue;
        }
        let _ = write!(
            out,
            "<span class=\"oo-chip\"><i class=\"oo-sw oo-{}\">{}</i>{}<b>{n}</b></span>",
            state.as_str(),
            state_icon(state),
            escape(state_label(state, labels))
        );
    }
    let _ = write!(
        out,
        "<span class=\"oo-chip\"><i class=\"oo-sw oo-pass oo-tentative\"></i>{}</span>\
         <span class=\"oo-chip\"><i class=\"oo-badge oo-badge-key\">n</i>{}</span></div>",
        escape(&labels.tentative),
        escape(&labels.failed_here)
    );

    let Some(x) = &m.x else {
        let _ = write!(
            out,
            "<p class=\"oo-note\">{}</p></section>",
            escape(&labels.no_attributes)
        );
        return;
    };
    let y_labels = m.y_labels();
    out.push_str(
        "<div class=\"oo-scroll\"><table class=\"oo-grid\"><thead><tr><th class=\"oo-corner\">",
    );
    if let Some(y) = &m.y {
        let _ = write!(
            out,
            "<span class=\"oo-axis oo-axis-y\">{}{} &darr;</span>",
            escape(&y.name),
            unit(&y.unit)
        );
    }
    let _ = write!(
        out,
        "<span class=\"oo-axis oo-axis-x\">{}{} &rarr;</span></th>",
        escape(&x.name),
        unit(&x.unit)
    );
    for l in &x.labels {
        let _ = write!(
            out,
            "<th class=\"oo-col{}\" scope=\"col\">{}</th>",
            if x.is_outside(l) { " oo-out-h" } else { "" },
            escape(&label_title(l, labels))
        );
    }
    out.push_str("</tr></thead><tbody>");
    for (r, row) in m.cells.iter().enumerate() {
        let yl = &y_labels[r];
        let _ = write!(
            out,
            "<tr><th class=\"oo-row{}\" scope=\"row\">{}</th>",
            if m.y.as_ref().is_some_and(|y| y.is_outside(yl)) {
                " oo-out-h"
            } else {
                ""
            },
            escape(&label_title(yl, labels))
        );
        for (c, cell) in row.iter().enumerate() {
            let conds = m.conditions_of(r, c);
            let mut place = String::new();
            if let Some(y) = &m.y {
                let _ = write!(place, "{}={}, ", y.name, label_title(yl, labels));
            }
            let _ = write!(place, "{}={}", x.name, label_title(&x.labels[c], labels));
            let target = if !cell.visited() || !has_depth(&conds) {
                Target::None
            } else {
                match open {
                    Some(open) => match open(r, c) {
                        Some(id) => Target::Open(id),
                        None => Target::None,
                    },
                    None => Target::Drill(&conds),
                }
            };
            out.push_str("<td>");
            cell_html(out, cell, &place, target, opts);
            out.push_str("</td>");
        }
        out.push_str("</tr>");
    }
    out.push_str("</tbody></table></div>");
    if truncated {
        let _ = write!(
            out,
            "<p class=\"oo-note\">{}</p>",
            escape(&labels.truncated.replace("{n}", &MAX_PANELS.to_string()))
        );
    }
    if m.unsampled > 0 {
        let _ = write!(
            out,
            "<p class=\"oo-note\">{} {}.</p>",
            m.unsampled,
            escape(&labels.unsampled_documents)
        );
    }
    out.push_str("</section>");
}

fn unit(u: &str) -> String {
    if u.is_empty() {
        String::new()
    } else {
        format!(" <span class=\"oo-unit\">{}</span>", escape(u))
    }
}

fn open_root(out: &mut String, opts: &Options, mode: &str) {
    let _ = write!(
        out,
        "<div class=\"oo-cov\" id=\"{}\" data-oo-mode=\"{mode}\"><div class=\"oo-head\"><h3>{}</h3><p class=\"oo-hint\">{}</p></div>",
        escape(opts.root_id()),
        escape(&opts.labels.title),
        escape(&opts.labels.click_hint)
    );
}

/// One matrix, host-driven: a click on a cell dispatches `openodd:drill`
/// (and follows [`Options::link`] if given); the host renders the next level.
pub fn fragment(data: &Dataset, m: &Matrix, opts: &Options) -> String {
    let mut out = String::new();
    open_root(&mut out, opts, "host");
    let has_depth = |conds: &[(String, String)]| !data.free_attributes(&m.query, conds).is_empty();
    let id = format!("{}-p0", opts.root_id());
    panel(false, &mut out, m, opts, &id, false, None, None, &has_depth);
    out.push_str("</div>");
    out
}

/// The most panels the explorer renders, whatever the depth: past it, a
/// cell's level is not rendered, and its panel says so.
pub const MAX_PANELS: usize = 400;

/// The matrix `query` asks for and, `depth` levels down, the matrix inside
/// each of its visited cells, as hidden panels a click shows. Nothing to
/// serve: the whole thing is one string. The levels below lay out the first
/// two attributes not yet clicked into; at most [`MAX_PANELS`] panels.
pub fn explorer(data: &Dataset, query: &Query, depth: usize, opts: &Options) -> String {
    let mut out = String::new();
    open_root(&mut out, opts, "explorer");
    let mut count = 0usize;
    let root = opts.root_id().to_string();
    let has_depth = |conds: &[(String, String)]| !data.free_attributes(query, conds).is_empty();
    // Breadth-first: (query, its panel id, the levels above, levels left).
    #[allow(clippy::type_complexity)]
    let mut queue: std::collections::VecDeque<(Query, String, Vec<(String, usize)>, usize)> =
        std::collections::VecDeque::new();
    queue.push_back((query.clone(), format!("{root}-p0"), Vec::new(), depth));
    count += 1;
    while let Some((q, id, crumbs, left)) = queue.pop_front() {
        let m = data.matrix(&q);
        let mut children: Vec<((usize, usize), String)> = Vec::new();
        let mut truncated = false;
        if left > 0 {
            for (r, row) in m.cells.iter().enumerate() {
                for (c, cell) in row.iter().enumerate() {
                    let conds = m.conditions_of(r, c);
                    if !cell.visited() || !has_depth(&conds) {
                        continue;
                    }
                    if count >= MAX_PANELS {
                        truncated = true;
                        continue;
                    }
                    let child = format!("{root}-p{count}");
                    count += 1;
                    let mut cq = q.clone();
                    cq.conditions = conds;
                    cq.x = None;
                    cq.y = None;
                    let mut up = crumbs.clone();
                    up.push((id.clone(), m.query.conditions.len()));
                    queue.push_back((cq, child.clone(), up, left - 1));
                    children.push(((r, c), child));
                }
            }
        }
        let open = |r: usize, c: usize| {
            children
                .iter()
                .find(|(rc, _)| *rc == (r, c))
                .map(|(_, id)| id.clone())
        };
        panel(
            truncated,
            &mut out,
            &m,
            opts,
            &id,
            !crumbs.is_empty(),
            Some(&open),
            Some(&crumbs),
            &has_depth,
        );
    }
    out.push_str("</div>");
    out
}

/// A standalone page: the styles, the script and `body`.
pub fn page(title: &str, body: &str) -> String {
    format!(
        "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <meta name=\"color-scheme\" content=\"light dark\"><title>{}</title>\
         <style>:root{{color-scheme:light dark}}body{{margin:0;padding:32px;background:#f2f4f7}}@media (prefers-color-scheme:dark){{body{{background:#0b0e12}}}}</style>\
         <style>{}</style></head><body>{}<script>{}</script></body></html>\n",
        escape(title),
        styles(),
        body,
        script()
    )
}

/// The component's CSS, scoped under `.oo-cov`. Put it on the page once.
pub fn styles() -> &'static str {
    include_str!("assets/coverage_view.css")
}

/// The component's script: dispatches `openodd:drill`, and shows the
/// explorer's panels. Put it on the page once; it is idempotent.
pub fn script() -> &'static str {
    include_str!("assets/coverage_view.js")
}
