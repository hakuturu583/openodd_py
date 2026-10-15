//! Two sets of runs in one case plot: every case at its values, its mark
//! split in two -- what its baseline runs did on the left, what its other
//! runs did on the right -- and ringed by how its failure rate moved
//! ([`crate::compare`]). Behind the marks, the change of the pass rate the
//! flaky model predicts (each set's own model), or either set's prediction.
//!
//! Host-driven like [`super::fragment`]: the same axis pickers (and drawn /
//! measured toggle) dispatch `openodd:axes` with `detail.view == "cases"`.

use std::fmt::Write as _;

use super::{
    axis_svg, contour_class, contour_labels, contours, layout, path_d, pct, pickers, short_number,
    value_of, verdict_label, AxesUi, Geometry, Layout, Options, COMPACT, CONTOURS, WIDE,
};
use crate::cases::{Basis, Case, Verdict};
use crate::compare::{CaseComparison, Change, ComparedCase, Comparison, Labels as Words, Side};
use crate::html::compare::{change_kind, change_text, named, odd_warning, points};
use crate::html::escape;
use crate::prediction::{Grid, Predicted, Predictor};

/// What the plot is shaded by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Background {
    /// The change of the predicted pass rate: the other set's model minus
    /// the baseline's.
    Delta,
    /// One set's predicted pass rate, as the case plot shows it.
    Side(Side),
    None,
}

impl Background {
    /// `"delta"`, `"base"`, `"other"` or `"none"`.
    pub fn parse(s: &str) -> Option<Background> {
        match s {
            "delta" => Some(Background::Delta),
            "base" => Some(Background::Side(Side::Base)),
            "other" => Some(Background::Side(Side::Other)),
            "none" => Some(Background::None),
            _ => None,
        }
    }
}

/// The radius of a compared mark: two marks side by side, in a ring.
const RING: f64 = 8.5;

/// The grid of the change: per cell, one half plus half the change of the
/// pass probability (0: the other set always fails where the baseline always
/// passed; 1: the reverse), on the plot's own cells.
pub fn delta_grid(base: &Grid, other: &Grid) -> Result<Grid, String> {
    if base.x != other.x || base.y != other.y {
        return Err("the two models' grids differ".into());
    }
    let cells = base
        .cells
        .iter()
        .zip(&other.cells)
        .map(|(a, b)| {
            a.iter()
                .zip(b)
                .map(|(a, b)| match (a, b) {
                    (Some(a), Some(b)) => Some(Predicted {
                        pass_probability: 0.5 + (b.pass_probability - a.pass_probability) / 2.0,
                        flip_probability: None,
                        instability_confidence: None,
                        state: None,
                    }),
                    _ => None,
                })
                .collect()
        })
        .collect();
    Ok(Grid {
        cells,
        runs: base.runs + other.runs,
        ..other.clone()
    })
}

/// One side's mark, small, at `dx` from the ring's centre; a dot when the
/// side did not run the case.
fn mini(out: &mut String, verdict: Option<Verdict>, dx: f64) {
    match verdict {
        Some(v) => {
            let _ = write!(
                out,
                "<g class=\"oo-case oo-{}\" transform=\"translate({dx:.1} 0)\">",
                v.as_str()
            );
            super::mark_shape(out, v, 3.4, false);
            out.push_str("</g>");
        }
        None => {
            let _ = write!(
                out,
                "<circle class=\"oo-cmp-none\" cx=\"{dx:.1}\" cy=\"0\" r=\"1.8\"/>"
            );
        }
    }
}

/// A compared mark at the origin: the ring of its change, and each side's
/// verdict.
fn glyph(out: &mut String, base: Option<Verdict>, other: Option<Verdict>, change: Change, r: f64) {
    let _ = write!(out, "<circle class=\"oo-chg-ring\" r=\"{r:.1}\"/>");
    mini(out, base, -4.3);
    mini(out, other, 4.3);
    // The direction, so the ring's colour does not say it alone: a
    // triangle pointing down (worse) or up (better).
    let (ax, ay) = (r * 0.78, -r * 0.78);
    let tip = match change {
        Change::Regressed => 1.0,
        Change::Improved => -1.0,
        _ => return,
    };
    let _ = write!(
        out,
        "<path class=\"oo-chg-arrow\" d=\"M{:.1},{:.1}L{:.1},{:.1}L{ax:.1},{:.1}Z\"/>",
        ax - 3.4,
        ay - 2.6 * tip,
        ax + 3.4,
        ay - 2.6 * tip,
        ay + 3.2 * tip
    );
}

fn verdicts(cc: &ComparedCase) -> (Option<Verdict>, Option<Verdict>) {
    (
        cc.base.as_ref().map(Case::verdict),
        cc.other.as_ref().map(Case::verdict),
    )
}

/// A compared mark as a legend key.
fn change_key(change: Change, significant: bool) -> String {
    let mut s = format!(
        "<svg class=\"oo-key\" viewBox=\"-12 -12 24 24\" width=\"22\" height=\"22\" aria-hidden=\"true\">\
         <g class=\"oo-cmp-case oo-chg-{}{}\">",
        change.as_str(),
        if significant { " oo-sig" } else { "" }
    );
    let (a, b) = match change {
        Change::Regressed => (Some(Verdict::Pass), Some(Verdict::Fail)),
        Change::Improved => (Some(Verdict::Fail), Some(Verdict::Pass)),
        Change::Same => (Some(Verdict::Pass), Some(Verdict::Pass)),
        Change::New => (None, Some(Verdict::Pass)),
        Change::Dropped => (Some(Verdict::Pass), None),
        Change::Neither => (None, None),
    };
    glyph(&mut s, a, b, change, RING);
    s.push_str("</g></svg>");
    s
}

/// One side's runs of a case, in words: its verdict, how many passed, and
/// each run in order.
fn side_text(case: Option<&Case>, opts: &Options, words: &Words) -> String {
    let Some(c) = case else {
        return words.not_run.clone();
    };
    let labels = &opts.labels;
    let seq: String = c
        .runs
        .iter()
        .take(12)
        .map(|r| {
            if r.passed {
                labels.pass_short.as_str()
            } else {
                labels.fail_short.as_str()
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "{} \u{b7} {} \u{b7} {seq}{}",
        verdict_label(c.verdict(), labels),
        labels
            .passes_of
            .replace("{p}", &c.passes().to_string())
            .replace("{n}", &c.runs.len().to_string()),
        if c.runs.len() > 12 { " \u{2026}" } else { "" }
    )
}

/// A case's card.
#[allow(clippy::too_many_arguments)]
fn card(
    cmp: &Comparison,
    case: &Case,
    cc: &ComparedCase,
    n: usize,
    scenarios: bool,
    model: (Option<f64>, Option<f64>),
    opts: &Options,
    words: &Words,
) -> serde_json::Value {
    let labels = &opts.labels;
    let mut place = labels.case.replace("{n}", &n.to_string());
    if scenarios {
        let _ = write!(place, " \u{b7} {}", case.scenario);
    }
    let mut rows: Vec<(String, String)> = Vec::new();
    for name in case.values.keys() {
        let mut text = value_of(case, cmp.cases.axes.get(name), name, true);
        if let Some(b) = case.buckets.get(name).filter(|b| **b != text) {
            let _ = write!(text, "  {b}");
        }
        rows.push((name.clone(), text));
    }
    rows.push((words.base.clone(), side_text(cc.base.as_ref(), opts, words)));
    rows.push((
        words.other.clone(),
        side_text(cc.other.as_ref(), opts, words),
    ));
    if model.0.is_some() || model.1.is_some() {
        let p = |x: Option<f64>| x.map(pct).unwrap_or_else(|| "\u{2013}".into());
        rows.push((
            labels.model_pass.clone(),
            format!("{} \u{2192} {}", p(model.0), p(model.1)),
        ));
    }
    let chosen: Vec<String> = case
        .chosen_for()
        .iter()
        .filter(|c| !c.is_empty())
        .map(|c| super::chosen_label(c, labels))
        .collect();
    if !chosen.is_empty() {
        rows.push((labels.chosen_for.clone(), chosen.join(", ")));
    }
    serde_json::json!({
        "place": place,
        "state": change_text(&cc.delta, words),
        "kind": change_kind(cc.delta.change),
        "tentative": None::<String>,
        "rows": rows,
        "wide": true,
    })
}

/// The comparison plot laid out at one size: marks a ring apart.
fn compared_layout(p: &CaseComparison, g: Geometry) -> Layout<'_> {
    layout(&p.plot, Geometry { r: RING + 0.5, ..g }, false)
}

#[allow(clippy::too_many_arguments)]
fn svg(
    out: &mut String,
    p: &CaseComparison,
    lay: &Layout,
    class: &str,
    shade: Option<(&Grid, &str)>,
    opts: &Options,
    words: &Words,
) {
    let g = &lay.g;
    let labels = &opts.labels;
    let plot = &p.plot;
    let (l, t, r, b) = lay.area;
    let name = match (&plot.x, &plot.y) {
        (Some(x), Some(y)) => format!(
            "{}: {} \u{d7} {}",
            named(&words.cases_title, words),
            x.name,
            y.name
        ),
        _ => named(&words.cases_title, words),
    };
    let _ = write!(
        out,
        "<svg class=\"oo-svg {class}\" viewBox=\"0 0 {} {}\" role=\"group\" aria-label=\"{}\">\
         <rect class=\"oo-frame\" x=\"{l:.1}\" y=\"{t:.1}\" width=\"{:.1}\" height=\"{:.1}\" rx=\"6\"/>",
        g.width,
        g.height,
        escape(&name),
        r - l,
        b - t
    );
    // Contours only on a set's own prediction: the change has no 30 / 70%.
    let found = match shade {
        Some((grid, "oo-pp-")) => contours(lay, grid),
        _ => Vec::new(),
    };
    if let Some((grid, scale)) = shade {
        super::background(out, lay, grid, scale);
    }
    axis_svg(out, lay, true, labels);
    axis_svg(out, lay, false, labels);
    if !found.is_empty() {
        out.push_str("<g class=\"oo-contours\" aria-hidden=\"true\">");
        for c in &found {
            for line in &c.lines {
                let d = path_d(line);
                let _ = write!(
                    out,
                    "<path class=\"oo-cl-halo\" d=\"{d}\"/><path class=\"oo-cl {}\" d=\"{d}\"/>",
                    contour_class(c.level)
                );
            }
        }
        out.push_str("</g>");
    }
    for m in lay.marks.iter().filter(|m| m.moved()) {
        let _ = write!(
            out,
            "<line class=\"oo-lead\" x1=\"{:.1}\" y1=\"{:.1}\" x2=\"{:.1}\" y2=\"{:.1}\"/>\
             <circle class=\"oo-dot\" cx=\"{:.1}\" cy=\"{:.1}\" r=\"1.6\"/>",
            m.anchor.0, m.anchor.1, m.at.0, m.at.1, m.anchor.0, m.anchor.1
        );
    }
    // The unchanged first, so what changed is drawn on top.
    let mut order: Vec<usize> = (0..plot.cases.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(p.cases[i].delta.change.rank()));
    for i in order {
        let (c, cc, m) = (&plot.cases[i], &p.cases[i], &lay.marks[i]);
        let mut text = [&plot.x, &plot.y]
            .into_iter()
            .flatten()
            .map(|a| format!("{}={}", a.name, value_of(c, Some(a), &a.name, false)))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = write!(
            text,
            ": {}; {} {}; {} {}",
            change_text(&cc.delta, words),
            words.base,
            side_text(cc.base.as_ref(), opts, words),
            words.other,
            side_text(cc.other.as_ref(), opts, words)
        );
        let _ = write!(
            out,
            "<g class=\"oo-case oo-cmp-case oo-chg-{}{}\" transform=\"translate({:.1} {:.1})\" tabindex=\"0\" \
             role=\"img\" aria-label=\"{}\" data-oo-tip-i=\"{i}\"><circle class=\"oo-hit\" r=\"{:.1}\"/>",
            cc.delta.change.as_str(),
            if cc.delta.significant { " oo-sig" } else { "" },
            m.at.0,
            m.at.1,
            escape(&text),
            RING + 4.0
        );
        let (a, b) = verdicts(cc);
        glyph(out, a, b, cc.delta.change, RING);
        out.push_str("</g>");
    }
    for label in contour_labels(lay, &found) {
        let _ = write!(
            out,
            "<text class=\"oo-cl-label {}\" x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"middle\" \
             dominant-baseline=\"central\" aria-hidden=\"true\">{}%</text>",
            contour_class(label.level),
            label.x,
            label.y,
            short_number(label.level * 100.0)
        );
    }
    out.push_str("</svg>");
}

/// The cases that changed: each one's change, values and runs on each side.
fn table(out: &mut String, p: &CaseComparison, opts: &Options, words: &Words) {
    let labels = &opts.labels;
    let plot = &p.plot;
    let _ = write!(
        out,
        "<div class=\"oo-case-table\"><h4>{}</h4>",
        escape(&words.changed_title)
    );
    let changed = p.changed();
    if changed.is_empty() {
        let _ = write!(
            out,
            "<p class=\"oo-note\">{}</p></div>",
            escape(&words.no_changed)
        );
        return;
    }
    let mut names: Vec<String> = [&plot.x, &plot.y]
        .into_iter()
        .flatten()
        .map(|a| a.name.clone())
        .collect();
    for &i in &changed {
        for k in plot.cases[i].values.keys() {
            if !names.contains(k) {
                names.push(k.clone());
            }
        }
    }
    let plotted = [&plot.x, &plot.y].into_iter().flatten().count();
    let _ = write!(
        out,
        "<div class=\"oo-scroll oo-table-scroll\"><table class=\"oo-table oo-cmp-table\"><thead><tr><th scope=\"col\">{}</th>",
        escape(&words.change)
    );
    for (j, n) in names.iter().enumerate() {
        let _ = write!(
            out,
            "<th scope=\"col\" class=\"{}\">{}</th>",
            if j < plotted { "oo-plotted" } else { "" },
            escape(n)
        );
    }
    let _ = write!(
        out,
        "<th scope=\"col\">{}</th><th scope=\"col\">{}</th><th scope=\"col\">{}</th></tr></thead><tbody>",
        escape(&words.base),
        escape(&words.other),
        escape(&words.fail_rate_change)
    );
    let seq = |out: &mut String, c: Option<&Case>| {
        out.push_str("<td><span class=\"oo-seq\">");
        match c {
            None => {
                let _ = write!(
                    out,
                    "<span class=\"oo-cmp-notrun\">{}</span>",
                    escape(&words.not_run)
                );
            }
            Some(c) => {
                for r in &c.runs {
                    let (cls, letter) = if r.passed {
                        ("pass", &labels.pass_short)
                    } else {
                        ("fail", &labels.fail_short)
                    };
                    let _ = write!(out, "<b class=\"oo-o oo-o-{cls}\">{}</b>", escape(letter));
                }
            }
        }
        out.push_str("</span></td>");
    };
    for i in changed {
        let (c, cc) = (&plot.cases[i], &p.cases[i]);
        let _ = write!(
            out,
            "<tr class=\"oo-cmp-row oo-chg-{}\"><th scope=\"row\" class=\"oo-v-cell\"><span class=\"oo-verdict\">{}{}</span></th>",
            cc.delta.change.as_str(),
            change_key(cc.delta.change, cc.delta.significant),
            escape(words.change_label(cc.delta.change))
        );
        for (j, n) in names.iter().enumerate() {
            let axis = [&plot.x, &plot.y]
                .into_iter()
                .flatten()
                .find(|a| &a.name == n);
            let _ = write!(
                out,
                "<td class=\"oo-val{}\">{}</td>",
                if j < plotted { " oo-plotted" } else { "" },
                escape(&value_of(c, axis, n, false))
            );
        }
        seq(out, cc.base.as_ref());
        seq(out, cc.other.as_ref());
        let _ = write!(
            out,
            "<td class=\"oo-val\">{}</td></tr>",
            cc.delta
                .fail_rate
                .map(points)
                .unwrap_or_else(|| "\u{2013}".into())
        );
    }
    out.push_str("</tbody></table></div></div>");
}

/// The comparison's case plot, host-driven: the axis pickers dispatch
/// `openodd:axes`; the host renders the plot asked for. `models` are the
/// two sides' predictors (the baseline's, the other's): the background
/// `background` asks for is drawn when the ones it needs give a grid, and
/// each card has both sides' predicted pass probability.
pub fn fragment(
    cmp: &Comparison,
    p: &CaseComparison,
    opts: &Options,
    words: &Words,
    background: Background,
    models: [Option<&dyn Predictor>; 2],
) -> String {
    let labels = &opts.labels;
    let plot = &p.plot;
    let mut out = String::new();
    let _ = write!(
        out,
        "<div class=\"oo-cov oo-cases oo-cmp\" id=\"{}\" data-oo-mode=\"host\"><div class=\"oo-head\"><h3>{}</h3>\
         <p class=\"oo-hint\">{}</p></div><section class=\"oo-panel\" id=\"{}-p0\" tabindex=\"-1\">",
        escape(opts.root_id()),
        escape(&named(&words.cases_title, words)),
        escape(&named(&words.cases_hint, words)),
        escape(opts.root_id())
    );
    let counts = p.counts();
    let stat = |out: &mut String, k: &str, v: String, sub: String, kind: &str| {
        let _ = write!(
            out,
            "<div class=\"oo-stat oo-stat-{kind}\"><span class=\"oo-k\">{}</span>\
             <span class=\"oo-v\">{v}</span><span class=\"oo-s\">{}</span></div>",
            escape(k),
            escape(&sub)
        );
    };
    out.push_str("<div class=\"oo-stats\">");
    for side in [Side::Base, Side::Other] {
        let cases = p
            .cases
            .iter()
            .filter(|c| match side {
                Side::Base => c.base.is_some(),
                Side::Other => c.other.is_some(),
            })
            .count();
        stat(
            &mut out,
            &format!("{} \u{b7} {}", labels.cases, words.side(side)),
            cases.to_string(),
            labels.runs_of.replace("{n}", &p.runs(side).to_string()),
            "runs",
        );
    }
    stat(
        &mut out,
        &words.regressed,
        counts["regressed"].to_string(),
        words.cases_suffix.clone(),
        "fail",
    );
    stat(
        &mut out,
        &words.improved,
        counts["improved"].to_string(),
        words.cases_suffix.clone(),
        "pass",
    );
    out.push_str("</div>");
    odd_warning(&mut out, &cmp.odd, words);
    let pooled = p
        .cases
        .iter()
        .flat_map(|c| [&c.base, &c.other])
        .flatten()
        .flat_map(|c| &c.runs)
        .map(|r| (cmp.side_of(r.document), &r.environment))
        .fold(Vec::new(), |mut seen, e| {
            if !seen.contains(&e) {
                seen.push(e);
            }
            seen
        });
    let per_side = |s: Side| pooled.iter().filter(|(x, _)| *x == s).count();
    if per_side(Side::Base) > 1 || per_side(Side::Other) > 1 {
        let _ = write!(
            out,
            "<p class=\"oo-warn\">{}</p>",
            escape(&labels.pooled_environments)
        );
    }
    let (Some(_), true) = (&plot.x, !plot.cases.is_empty()) else {
        let _ = write!(
            out,
            "<p class=\"oo-note\">{}</p></section></div>",
            escape(&labels.no_attributes)
        );
        return out;
    };
    let wide = compared_layout(p, WIDE);
    let compact = compared_layout(p, COMPACT);
    // The background.
    let side_grid = |s: Side| {
        let i = if s == Side::Base { 0 } else { 1 };
        models[i].map(|m| m.grid(plot))
    };
    let grid: Option<Result<Grid, String>> = match background {
        Background::None => None,
        Background::Side(s) => side_grid(s),
        Background::Delta => match (side_grid(Side::Base), side_grid(Side::Other)) {
            (Some(Ok(a)), Some(Ok(b))) => Some(delta_grid(&a, &b)),
            (Some(Err(e)), _) | (_, Some(Err(e))) => Some(Err(e)),
            _ => None,
        },
    };
    let scale = if background == Background::Delta {
        "oo-pd-"
    } else {
        "oo-pp-"
    };
    let shade = match &grid {
        Some(Ok(g)) => Some((g, scale)),
        _ => None,
    };
    out.push_str("<div class=\"oo-legend oo-case-legend\">");
    let _ = write!(
        out,
        "<span class=\"oo-chip oo-cmp-sides\">{}{}</span>",
        change_key(Change::Same, false),
        escape(&named(&words.sides_key, words))
    );
    for change in Change::ALL {
        let n = counts[change.as_str()];
        if n == 0 && matches!(change, Change::Neither | Change::Same) {
            continue;
        }
        let _ = write!(
            out,
            "<span class=\"oo-chip\">{}{}<b>{n}</b></span>",
            change_key(change, true),
            escape(words.change_label(change)),
        );
    }
    let _ = write!(
        out,
        "<span class=\"oo-chip\">{}{}</span>",
        change_key(Change::Regressed, false),
        escape(&words.within_noise)
    );
    match shade {
        Some((_, "oo-pd-")) => {
            let _ = write!(
                out,
                "<span class=\"oo-chip oo-pred-legend\">{}<span class=\"oo-pred-scale\" aria-hidden=\"true\">\
                 <i class=\"oo-pred-bar oo-delta-bar\"></i><span class=\"oo-pred-ticks\"><span>\u{2212}100% {}</span>\
                 <span>0</span><span>+100% {}</span></span></span></span>",
                escape(&words.delta_scale),
                escape(&words.delta_worse),
                escape(&words.delta_better),
            );
        }
        Some(_) => {
            let _ = write!(
                out,
                "<span class=\"oo-chip oo-pred-legend\">{}<span class=\"oo-pred-scale\" aria-hidden=\"true\">\
                 <i class=\"oo-pred-bar\"></i><span class=\"oo-pred-ticks\"><span>0% {}</span><span>50% {}</span>\
                 <span>100% {}</span></span></span></span>",
                escape(&labels.predicted_scale),
                escape(&labels.predicted_low),
                escape(&labels.predicted_mid),
                escape(&labels.predicted_high),
            );
            for level in CONTOURS {
                let value = format!("{}%", short_number(level * 100.0));
                let _ = write!(
                    out,
                    "<span class=\"oo-chip\"><svg class=\"oo-key\" viewBox=\"0 0 22 12\" width=\"22\" height=\"12\" aria-hidden=\"true\">\
                     <path class=\"oo-cl-halo\" d=\"M1 6H21\"/><path class=\"oo-cl {}\" d=\"M1 6H21\"/></svg>{}</span>",
                    contour_class(level),
                    escape(&labels.predicted_contours.replace("{p}", &value))
                );
            }
        }
        None => {}
    }
    if wide.marks.iter().chain(&compact.marks).any(|m| m.moved()) {
        let _ = write!(
            out,
            "<span class=\"oo-chip\"><svg class=\"oo-key\" viewBox=\"0 0 18 18\" width=\"18\" height=\"18\" aria-hidden=\"true\">\
             <line class=\"oo-lead\" x1=\"3\" y1=\"14\" x2=\"12\" y2=\"6\"/><circle class=\"oo-dot\" cx=\"3\" cy=\"14\" r=\"1.6\"/></svg>{}</span>",
            escape(&labels.moved)
        );
    }
    out.push_str("</div>");
    let toggle = plot.cases.iter().any(|c| !c.other.is_empty());
    pickers(
        &mut out,
        plot,
        &plot.attributes,
        &AxesUi::Host,
        opts,
        toggle,
    );
    out.push_str("<div class=\"oo-plot\">");
    svg(&mut out, p, &wide, "oo-plot-wide", shade, opts, words);
    svg(&mut out, p, &compact, "oo-plot-compact", shade, opts, words);
    out.push_str("</div>");
    match &grid {
        Some(Ok(g)) if background == Background::Delta => {
            let _ = write!(
                out,
                "<p class=\"oo-note oo-pred-note\">{}",
                escape(&named(
                    &words
                        .delta_caption
                        .replace("{base_runs}", &p.runs(Side::Base).to_string())
                        .replace("{other_runs}", &p.runs(Side::Other).to_string()),
                    words
                ))
            );
            held(&mut out, g, opts);
            out.push_str("</p>");
        }
        Some(Ok(g)) => {
            let _ = write!(
                out,
                "<p class=\"oo-note oo-pred-note\">{}",
                escape(
                    &labels
                        .predicted_caption
                        .replace("{runs}", &g.runs.to_string())
                )
            );
            held(&mut out, g, opts);
            out.push_str("</p>");
        }
        Some(Err(reason)) if !reason.is_empty() => {
            let _ = write!(
                out,
                "<p class=\"oo-note oo-pred-note\">{}</p>",
                escape(&labels.no_prediction.replace("{reason}", reason))
            );
        }
        _ => {}
    }
    if plot.query.basis == Basis::Measured {
        let _ = write!(
            out,
            "<p class=\"oo-note\">{}</p>",
            escape(
                &labels
                    .measured_note
                    .replace("{s}", &short_number(plot.query.window()))
            )
        );
    }
    if opts.table {
        table(&mut out, p, opts, words);
    }
    if plot.unplaced > 0 {
        let _ = write!(
            out,
            "<p class=\"oo-note\">{} {}.</p>",
            plot.unplaced,
            escape(&labels.unplaced)
        );
    }
    let skipped: usize = p.skipped.iter().map(|(a, b)| a + b).sum();
    if skipped > 0 {
        let _ = write!(
            out,
            "<p class=\"oo-note\">{skipped} {}.</p>",
            escape(&labels.skipped)
        );
    }
    out.push_str("</section>");
    // The cards, by the marks' index.
    let scenarios = plot
        .cases
        .iter()
        .any(|c| c.scenario != plot.cases[0].scenario);
    let refs: Vec<&Case> = plot.cases.iter().collect();
    let probs = |i: usize| match models[i] {
        Some(m) => m.pass_probabilities(&refs),
        None => vec![None; refs.len()],
    };
    let (pa, pb) = (probs(0), probs(1));
    let cards: Vec<serde_json::Value> = plot
        .cases
        .iter()
        .zip(&p.cases)
        .enumerate()
        .map(|(i, (c, cc))| card(cmp, c, cc, i + 1, scenarios, (pa[i], pb[i]), opts, words))
        .collect();
    let _ = write!(
        out,
        "<script type=\"application/json\" class=\"oo-tips\">{}</script></div>",
        crate::html::script_json(&serde_json::Value::Array(cards))
    );
    out
}

/// The attributes the background holds, after its caption.
fn held(out: &mut String, g: &Grid, opts: &Options) {
    if g.held.is_empty() {
        return;
    }
    let held: Vec<String> = g.held.iter().map(|(a, v)| format!("{a} = {v}")).collect();
    let _ = write!(
        out,
        " {}",
        escape(&opts.labels.held.replace("{held}", &held.join(", ")))
    );
}
