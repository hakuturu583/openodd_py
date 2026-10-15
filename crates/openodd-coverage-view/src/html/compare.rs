//! Two sets of runs on one ODD matrix: each cell split in two, the
//! baseline's runs on the left and the other set's on the right, outlined by
//! how the failure rate moved ([`crate::compare`]).
//!
//! The fragment is host-driven like [`super::fragment`]: the same
//! breadcrumbs, axis pickers and cells dispatch `openodd:drill` and
//! `openodd:axes` (or follow [`super::Options::link`]); the host renders the
//! comparison of the level asked for.

use std::fmt::Write as _;

use super::{
    axes_parts, crumbs_html, escape, label_title, pct, state_icon, state_label, target_attrs,
    AxesUi, Options, Target,
};
use crate::compare::{
    Change, Comparison, Delta, Labels as Words, MatrixComparison, OddCheck, Side,
};
use crate::matrix::{Cell, CellState};

/// `{base}` and `{other}` in `text`, as the sides' names.
pub(crate) fn named(text: &str, words: &Words) -> String {
    text.replace("{base}", &words.base)
        .replace("{other}", &words.other)
}

/// A change of failure rate in percentage points, signed: `+40 pt`.
pub(crate) fn points(d: f64) -> String {
    let p = (d * 100.0).round();
    if p == 0.0 {
        "\u{b1}0 pt".into()
    } else {
        format!("{}{} pt", if p > 0.0 { "+" } else { "\u{2212}" }, p.abs())
    }
}

/// The arrow of a change: down for worse, up for better.
pub(crate) fn arrow(change: Change) -> &'static str {
    match change {
        Change::Regressed => "\u{25bc}",
        Change::Improved => "\u{25b2}",
        Change::New => "+",
        Change::Dropped => "\u{2212}",
        Change::Same => "=",
        Change::Neither => "",
    }
}

/// What the tip's dot shows for a change: its state colour.
pub(crate) fn change_kind(change: Change) -> &'static str {
    match change {
        Change::Regressed => "fail",
        Change::Improved => "pass",
        Change::New | Change::Dropped => "unknown",
        Change::Same | Change::Neither => "unsampled",
    }
}

/// The change, in words, with whether it is beyond the runs' noise.
pub(crate) fn change_text(d: &Delta, words: &Words) -> String {
    let mut text = words.change_label(d.change).to_string();
    if let Some(f) = d.fail_rate {
        let _ = write!(text, " {}", points(f));
    }
    if matches!(d.change, Change::Regressed | Change::Improved) {
        let _ = write!(
            text,
            " ({})",
            if d.significant {
                &words.significant
            } else {
                &words.within_noise
            }
        );
    }
    text
}

/// The warning when the two sets were not measured on the same ODD.
pub(crate) fn odd_warning(out: &mut String, odd: &OddCheck, words: &Words) {
    if odd.same() {
        return;
    }
    let mut what: Vec<String> = Vec::new();
    for a in &odd.attributes {
        what.push(match a.only_in {
            Some(side) => words
                .odd_only_in
                .replace("{attribute}", &a.attribute)
                .replace("{side}", words.side(side)),
            None => words.odd_attribute.replace("{attribute}", &a.attribute),
        });
    }
    let _ = write!(
        out,
        "<p class=\"oo-warn oo-cmp-odd\">{}",
        escape(&words.different_odd)
    );
    if !what.is_empty() {
        let _ = write!(out, " {}.", escape(&what.join("; ")));
    }
    out.push_str("</p>");
}

/// One side's half of a cell.
fn half(out: &mut String, c: &Cell, side: Side) {
    let _ = write!(
        out,
        "<span class=\"oo-half oo-half-{} oo-{}{}\">",
        side.as_str(),
        c.state.as_str(),
        if c.tentative { " oo-tentative" } else { "" }
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
    out.push_str("</span>");
}

/// A side's line in a cell's card: its state, failures and interval.
fn side_row(c: &Cell, opts: &Options, words: &Words) -> String {
    if c.runs == 0 {
        return if c.runs_unknown > 0 {
            format!(
                "{} ({})",
                state_label(c.state, &opts.labels),
                c.runs_unknown
            )
        } else {
            words.not_run.clone()
        };
    }
    let (lo, hi) = c.interval().unwrap_or((0.0, 1.0));
    format!(
        "{} \u{b7} {}/{} ({}; {}\u{2013}{})",
        state_label(c.state, &opts.labels),
        c.failed,
        c.runs,
        pct(c.fail_rate().unwrap_or(0.0)),
        pct(lo),
        pct(hi)
    )
}

#[allow(clippy::too_many_arguments)]
fn cell_html(
    out: &mut String,
    base: &Cell,
    other: &Cell,
    d: &Delta,
    outside: bool,
    place: &str,
    target: Target,
    opts: &Options,
    words: &Words,
) {
    let (attrs, tag) = target_attrs(&target, opts);
    let mut class = format!("oo-cell oo-cmp-cell oo-chg-{}", d.change.as_str());
    if d.significant {
        class += " oo-sig";
    }
    if outside {
        class += " oo-outside";
    }
    let tag_attrs = if tag == "button" {
        " type=\"button\""
    } else {
        ""
    };
    let change = change_text(d, words);
    let rows = vec![
        (words.base.clone(), side_row(base, opts, words)),
        (words.other.clone(), side_row(other, opts, words)),
        (words.change.clone(), change.clone()),
    ];
    let mut text = format!("{place}: {change}");
    for (k, v) in &rows[..2] {
        let _ = write!(text, ", {k} {v}");
    }
    let state = if outside {
        format!("{} \u{b7} {}", opts.labels.state_outside, change)
    } else {
        change
    };
    let tip = serde_json::json!({
        "place": place,
        "state": state,
        "kind": if outside { "outside" } else { change_kind(d.change) },
        "tentative": None::<String>,
        "rows": rows,
    });
    let _ = write!(
        out,
        "<{tag}{tag_attrs} class=\"{class}\" aria-label=\"{}\" data-oo-tip=\"{}\"{attrs}>",
        escape(&text),
        escape(&tip.to_string())
    );
    half(out, base, Side::Base);
    half(out, other, Side::Other);
    if d.change != Change::Neither && !outside {
        let _ = write!(
            out,
            "<span class=\"oo-chg-badge\" aria-hidden=\"true\">{}{}</span>",
            arrow(d.change),
            d.fail_rate
                .filter(|_| d.change != Change::Same)
                .map(|f| format!(" {} pt", (f.abs() * 100.0).round()))
                .unwrap_or_default()
        );
    }
    let _ = write!(out, "</{tag}>");
}

/// The legend of the changes, with each count.
pub(crate) fn change_legend(
    out: &mut String,
    counts: &indexmap::IndexMap<&'static str, usize>,
    words: &Words,
) {
    for change in Change::ALL {
        let n = counts[change.as_str()];
        if n == 0 && matches!(change, Change::Neither | Change::Same) {
            continue;
        }
        let _ = write!(
            out,
            "<span class=\"oo-chip\"><i class=\"oo-sw oo-chg-sw oo-chg-{}\">{}</i>{}<b>{n}</b></span>",
            change.as_str(),
            arrow(change),
            escape(words.change_label(change))
        );
    }
    let _ = write!(
        out,
        "<span class=\"oo-chip\"><i class=\"oo-sw oo-chg-sw oo-chg-regressed oo-sig\"></i>{}</span>\
         <span class=\"oo-chip\"><i class=\"oo-sw oo-chg-sw oo-chg-regressed\"></i>{}</span>",
        escape(&words.significant),
        escape(&words.within_noise)
    );
}

/// The comparison of one matrix, host-driven: a click on a cell dispatches
/// `openodd:drill` (and follows [`Options::link`] if given), the pickers
/// `openodd:axes`; the host renders the comparison asked for.
pub fn fragment(cmp: &Comparison, m: &MatrixComparison, opts: &Options, words: &Words) -> String {
    let labels = &opts.labels;
    let mut out = String::new();
    let _ = write!(
        out,
        "<div class=\"oo-cov oo-cmp\" id=\"{}\" data-oo-mode=\"host\"><div class=\"oo-head\"><h3>{}</h3>\
         <p class=\"oo-hint\">{}</p></div>",
        escape(opts.root_id()),
        escape(&named(&words.matrix_title, words)),
        escape(&named(&words.matrix_hint, words))
    );
    let u = &m.union;
    let _ = write!(
        out,
        "<section class=\"oo-panel\" id=\"{}-p0\" tabindex=\"-1\">",
        escape(opts.root_id())
    );
    crumbs_html(&mut out, &u.query.conditions, None, opts);
    let counts = m.counts();
    let stat = |out: &mut String, key: &str, value: String, sub: String, kind: &str| {
        let _ = write!(
            out,
            "<div class=\"oo-stat oo-stat-{kind}\"><span class=\"oo-k\">{}</span>\
             <span class=\"oo-v\">{value}</span><span class=\"oo-s\">{}</span></div>",
            escape(key),
            escape(&sub)
        );
    };
    out.push_str("<div class=\"oo-stats\">");
    for (side, mm) in [(Side::Base, &m.base), (Side::Other, &m.other)] {
        let t = &mm.total;
        stat(
            &mut out,
            &format!("{} \u{b7} {}", labels.runs, words.side(side)),
            (t.runs + t.runs_unknown).to_string(),
            format!(
                "{} {}{}",
                t.failed,
                labels.failed.to_lowercase(),
                t.fail_rate()
                    .map(|r| format!(" ({})", pct(r)))
                    .unwrap_or_default()
            ),
            "runs",
        );
    }
    stat(
        &mut out,
        &words.regressed,
        counts["regressed"].to_string(),
        words.cells_suffix.clone(),
        "fail",
    );
    stat(
        &mut out,
        &words.improved,
        counts["improved"].to_string(),
        words.cells_suffix.clone(),
        "pass",
    );
    stat(
        &mut out,
        &words.coverage_change,
        format!("+{} / \u{2212}{}", counts["new"], counts["dropped"]),
        words
            .coverage_change_sub
            .replace("{new}", &counts["new"].to_string())
            .replace("{dropped}", &counts["dropped"].to_string()),
        "coverage",
    );
    out.push_str("</div>");
    odd_warning(&mut out, &cmp.odd, words);
    for mm in [&m.base, &m.other] {
        if mm.environments.len() > 1 {
            let _ = write!(
                out,
                "<p class=\"oo-warn\">{}</p>",
                escape(&labels.pooled_environments)
            );
            break;
        }
    }
    out.push_str("<div class=\"oo-legend\">");
    let _ = write!(
        out,
        "<span class=\"oo-chip oo-cmp-sides\"><i class=\"oo-sw oo-cmp-key\" aria-hidden=\"true\"><b></b><b></b></i>{}</span>",
        escape(&named(&words.sides_key, words))
    );
    change_legend(&mut out, &counts, words);
    out.push_str("</div>");
    let Some(x) = &u.x else {
        let _ = write!(
            out,
            "<p class=\"oo-note\">{}</p></section></div>",
            escape(&labels.no_attributes)
        );
        return out;
    };
    let free = cmp.coverage.free_attributes(&u.query, &u.query.conditions);
    let has_depth =
        |conds: &[(String, String)]| !cmp.coverage.free_attributes(&u.query, conds).is_empty();
    let y_labels = u.y_labels();
    let parts = axes_parts(u, &free, &AxesUi::Host, opts);
    let _ = write!(
        out,
        "<div class=\"oo-scroll\"><table class=\"oo-grid oo-cmp-grid{}\"{}><thead><tr>\
         <th class=\"oo-corner\">{}</th>\
         <th class=\"oo-xhead\" colspan=\"{}\" scope=\"colgroup\">{}</th></tr>\
         <tr><th class=\"oo-corner-2\" aria-hidden=\"true\"></th>",
        if parts.attrs.is_empty() {
            ""
        } else {
            " oo-axes"
        },
        parts.attrs,
        parts.rows,
        x.labels.len(),
        parts.columns
    );
    for (c, l) in x.labels.iter().enumerate() {
        let _ = write!(
            out,
            "<th class=\"oo-col{}\" scope=\"col\" data-oo-c=\"{c}\">{}</th>",
            if x.is_outside(l) { " oo-out-h" } else { "" },
            escape(&label_title(l, labels))
        );
    }
    out.push_str("</tr></thead><tbody>");
    for (r, row) in m.deltas.iter().enumerate() {
        let yl = &y_labels[r];
        let _ = write!(
            out,
            "<tr><th class=\"oo-row{}\" scope=\"row\" data-oo-r=\"{r}\">{}</th>",
            if u.y.as_ref().is_some_and(|y| y.is_outside(yl)) {
                " oo-out-h"
            } else {
                ""
            },
            escape(&label_title(yl, labels))
        );
        for (c, d) in row.iter().enumerate() {
            let (a, b) = (&m.base.cells[r][c], &m.other.cells[r][c]);
            let conds = u.conditions_of(r, c);
            let mut place = String::new();
            if let Some(y) = &u.y {
                let _ = write!(place, "{}={}, ", y.name, label_title(yl, labels));
            }
            let _ = write!(place, "{}={}", x.name, label_title(&x.labels[c], labels));
            let target = if (a.visited() || b.visited()) && has_depth(&conds) {
                Target::Drill(&conds)
            } else {
                Target::None
            };
            let _ = write!(out, "<td data-oo-r=\"{r}\" data-oo-c=\"{c}\">");
            let outside = u.cells[r][c].state == CellState::Outside;
            cell_html(&mut out, a, b, d, outside, &place, target, opts, words);
            out.push_str("</td>");
        }
        out.push_str("</tr>");
    }
    out.push_str("</tbody></table></div>");
    let unsampled = u.unsampled;
    if unsampled > 0 {
        let _ = write!(
            out,
            "<p class=\"oo-note\">{unsampled} {}.</p>",
            escape(&labels.unsampled_documents)
        );
    }
    out.push_str("</section></div>");
    out
}
