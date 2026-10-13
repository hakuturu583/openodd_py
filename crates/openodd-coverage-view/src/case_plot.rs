//! The drawn cases as HTML: a scatter of a sweep, and a table of the cases
//! run more than once.
//!
//! Each distinct case ([`crate::cases::Case`]) is one mark, at the values it
//! was drawn with, along any two attributes of its space:
//!
//! * a numeric attribute on a linear scale over its range, with faint grid
//!   lines at its ODD bucket edges and the buckets named along the axis;
//! * a categorical one as evenly spaced bands, the marks jittered inside.
//!
//! A case that passed is a filled circle, one that failed a cross, and one
//! that did both a ring in the flaky colour; a case run more than once is
//! labelled `passes/runs`. A case a flaky search chose (`chosen_for:
//! instability`) has a dashed outline. Marks that would overlap are moved
//! apart, with a leader line to the value they stand for, and the labels are
//! placed where they hide no mark.
//!
//! The component shares the coverage view's root class (`.oo-cov`), styles,
//! script and colours (`--oo-pass`, `--oo-fail`, `--oo-flaky`, ...): put
//! [`crate::html::styles`] and [`crate::html::script`] on the page once for
//! both. The axis pickers dispatch the same cancelable `openodd:axes` event
//! (with `detail.view == "cases"`), or follow [`Options::link`]; the
//! [`explorer`] renders every pair of attributes ahead of time instead.

use std::collections::HashMap;
use std::fmt::Write as _;

use crate::cases::{AxisKind, Case, CaseAxis, CaseQuery, CaseSet, Plot, Verdict};
use crate::html::{escape, MAX_AXIS_PAIRS};
use crate::prediction::{Grid, GridAxis, Predictor};

/// The words the component shows, for a host to translate.
#[derive(Debug, Clone, PartialEq)]
pub struct Labels {
    pub title: String,
    pub hint: String,
    pub cases: String,
    pub runs_of: String,
    pub repeated: String,
    pub repeated_sub: String,
    pub flaky_cases: String,
    pub failing_cases: String,
    pub of_cases: String,
    pub state_pass: String,
    pub state_fail: String,
    pub state_flaky: String,
    pub instability: String,
    pub ratio: String,
    pub moved: String,
    pub x: String,
    pub y: String,
    pub no_attributes: String,
    pub skipped: String,
    pub unplaced: String,
    pub pooled_environments: String,
    pub table_title: String,
    pub no_repeated: String,
    pub verdict: String,
    pub runs: String,
    pub outcomes: String,
    pub pass_short: String,
    pub fail_short: String,
    pub passed: String,
    pub failed: String,
    pub failed_at: String,
    pub run: String,
    pub passes_of: String,
    pub more_runs: String,
    pub chosen_for: String,
    pub chosen_coverage: String,
    pub chosen_instability: String,
    pub pass_probability: String,
    pub flip_probability: String,
    pub instability_confidence: String,
    pub robustness_radius: String,
    pub scenario: String,
    pub case: String,
    pub predicted_pass: String,
    pub predicted_fail: String,
    pub predicted_flaky: String,
    pub predicted_caption: String,
    pub held: String,
    pub no_prediction: String,
    pub model_pass: String,
}

impl Labels {
    /// Set one label by its field name; `false` if there is no such label.
    pub fn set(&mut self, name: &str, value: &str) -> bool {
        let slot = match name {
            "title" => &mut self.title,
            "hint" => &mut self.hint,
            "cases" => &mut self.cases,
            "runs_of" => &mut self.runs_of,
            "repeated" => &mut self.repeated,
            "repeated_sub" => &mut self.repeated_sub,
            "flaky_cases" => &mut self.flaky_cases,
            "failing_cases" => &mut self.failing_cases,
            "of_cases" => &mut self.of_cases,
            "state_pass" => &mut self.state_pass,
            "state_fail" => &mut self.state_fail,
            "state_flaky" => &mut self.state_flaky,
            "instability" => &mut self.instability,
            "ratio" => &mut self.ratio,
            "moved" => &mut self.moved,
            "x" => &mut self.x,
            "y" => &mut self.y,
            "no_attributes" => &mut self.no_attributes,
            "skipped" => &mut self.skipped,
            "unplaced" => &mut self.unplaced,
            "pooled_environments" => &mut self.pooled_environments,
            "table_title" => &mut self.table_title,
            "no_repeated" => &mut self.no_repeated,
            "verdict" => &mut self.verdict,
            "runs" => &mut self.runs,
            "outcomes" => &mut self.outcomes,
            "pass_short" => &mut self.pass_short,
            "fail_short" => &mut self.fail_short,
            "passed" => &mut self.passed,
            "failed" => &mut self.failed,
            "failed_at" => &mut self.failed_at,
            "run" => &mut self.run,
            "passes_of" => &mut self.passes_of,
            "more_runs" => &mut self.more_runs,
            "chosen_for" => &mut self.chosen_for,
            "chosen_coverage" => &mut self.chosen_coverage,
            "chosen_instability" => &mut self.chosen_instability,
            "pass_probability" => &mut self.pass_probability,
            "flip_probability" => &mut self.flip_probability,
            "instability_confidence" => &mut self.instability_confidence,
            "robustness_radius" => &mut self.robustness_radius,
            "scenario" => &mut self.scenario,
            "case" => &mut self.case,
            "predicted_pass" => &mut self.predicted_pass,
            "predicted_fail" => &mut self.predicted_fail,
            "predicted_flaky" => &mut self.predicted_flaky,
            "predicted_caption" => &mut self.predicted_caption,
            "held" => &mut self.held,
            "no_prediction" => &mut self.no_prediction,
            "model_pass" => &mut self.model_pass,
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
            title: s("Drawn cases"),
            hint: s("Each mark is a case at the values it was drawn with"),
            cases: s("Cases"),
            runs_of: s("{n} runs"),
            repeated: s("Repeated"),
            repeated_sub: s("run more than once"),
            flaky_cases: s("Flaky cases"),
            failing_cases: s("Failing cases"),
            of_cases: s("of {n} cases"),
            state_pass: s("Passes"),
            state_fail: s("Fails"),
            state_flaky: s("Flaky: both outcomes"),
            instability: s("Chosen for instability"),
            ratio: s("passed / runs"),
            moved: s("Moved apart to stay legible; the line leads to its value"),
            x: s("X"),
            y: s("Y"),
            no_attributes: s("No case with drawn values to lay out."),
            skipped: s("coverage files without a case or an outcome are not shown"),
            unplaced: s("cases without a value of an axis are not shown"),
            pooled_environments: s(
                "Runs of different environments are pooled here; select one to tell flakiness from regressions.",
            ),
            table_title: s("Repeated cases"),
            no_repeated: s("No case was run more than once."),
            verdict: s("Verdict"),
            runs: s("Runs"),
            outcomes: s("Outcomes"),
            pass_short: s("P"),
            fail_short: s("F"),
            passed: s("Passed"),
            failed: s("Failed"),
            failed_at: s("Failed at {t} s"),
            run: s("Run {n}"),
            passes_of: s("{p} of {n} passed"),
            more_runs: s("+{n} more"),
            chosen_for: s("Chosen for"),
            chosen_coverage: s("coverage"),
            chosen_instability: s("instability"),
            pass_probability: s("Predicted pass probability"),
            flip_probability: s("Flip probability"),
            instability_confidence: s("Instability confidence"),
            robustness_radius: s("Robustness radius"),
            scenario: s("Scenario"),
            case: s("Case {n}"),
            predicted_pass: s("Predicted: passes"),
            predicted_fail: s("Predicted: fails"),
            predicted_flaky: s("Predicted: flaky"),
            predicted_caption: s(
                "Background: what the flaky model fitted to these {runs} runs predicts. Flaky where it is \
                 over {confidence} sure a small change flips the outcome more than {threshold} of the time; \
                 else passes where P(pass) is at least 50%, fails where below. Where runs are few the \
                 model is unsure, and unsure reads as flaky.",
            ),
            held: s("Attributes not plotted are held at the cases' typical values: {held}."),
            no_prediction: s("No predicted background: {reason}."),
            model_pass: s("Pass probability (model, now)"),
        }
    }
}

/// How the component is rendered.
#[derive(Debug, Clone, PartialEq)]
pub struct Options {
    /// The id of the component's root element (and the prefix of its panels'
    /// ids); `oo-cases` when empty.
    pub id: String,
    /// A URL template the axis pickers go to: `{axes}` (or `{conditions}`)
    /// is replaced by `x=...&y=...`.
    pub link: Option<String>,
    /// Whether a plot is followed by the table of repeated cases.
    pub table: bool,
    /// Whether a case chosen for instability gets a dashed outline (off: its
    /// card and the table still say why it was chosen).
    pub instability_rings: bool,
    pub labels: Labels,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            id: String::new(),
            link: None,
            table: true,
            instability_rings: false,
            labels: Labels::default(),
        }
    }
}

impl Options {
    fn root_id(&self) -> &str {
        if self.id.is_empty() {
            "oo-cases"
        } else {
            &self.id
        }
    }
}

// ---------------------------------------------------------------------------
// Numbers
// ---------------------------------------------------------------------------

fn trim(s: String) -> String {
    let s = if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s
    };
    if s == "-0" {
        "0".into()
    } else {
        s
    }
}

/// A value for a label or a table: as many decimals as its size needs.
pub fn short_number(v: f64) -> String {
    let a = v.abs();
    let s = if a >= 1000.0 || v.fract() == 0.0 {
        format!("{v:.0}")
    } else if a >= 10.0 {
        format!("{v:.1}")
    } else if a >= 1.0 {
        format!("{v:.2}")
    } else {
        format!("{v:.3}")
    };
    trim(s)
}

/// A value for the card: three decimals at most.
fn long_number(v: f64) -> String {
    trim(format!("{v:.3}"))
}

fn pct(x: f64) -> String {
    format!("{:.0}%", x * 100.0)
}

/// A case's value of an attribute, as text.
fn value_of(case: &Case, axis: Option<&CaseAxis>, attribute: &str, long: bool) -> String {
    let numeric = axis.is_some_and(|a| a.kind == AxisKind::Numeric);
    match case.values.get(attribute) {
        None => "\u{2013}".into(),
        Some(serde_json::Value::Number(n)) if numeric || n.is_f64() => match n.as_f64() {
            Some(v) if long => long_number(v),
            Some(v) => short_number(v),
            None => n.to_string(),
        },
        Some(v) => crate::cases::value_text(v),
    }
}

/// Up to `n` evenly spaced round numbers inside `lo..=hi`.
fn nice_ticks(lo: f64, hi: f64, n: usize) -> Vec<f64> {
    let span = hi - lo;
    if span.is_nan() || span <= 0.0 {
        return vec![lo];
    }
    let raw = span / n.max(1) as f64;
    let mag = 10f64.powf(raw.log10().floor());
    let step = [1.0, 2.0, 2.5, 5.0, 10.0]
        .iter()
        .map(|m| m * mag)
        .find(|s| span / s <= n as f64)
        .unwrap_or(10.0 * mag);
    let mut out = Vec::new();
    let mut t = (lo / step).ceil() * step;
    while t <= hi + step * 1e-9 {
        out.push(if t.abs() < step * 1e-9 { 0.0 } else { t });
        t += step;
    }
    out
}

/// A deterministic number in `[0, 1)` from a text (FNV-1a).
fn unit_hash(s: &str, salt: u64) -> f64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325 ^ salt;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    (h >> 11) as f64 / (1u64 << 53) as f64
}

/// Text cut to fit `px` at `font`, with an ellipsis.
fn fit(text: &str, px: f64, font: f64) -> String {
    let max = (px / (font * 0.6)).floor().max(1.0) as usize;
    let n = text.chars().count();
    if n <= max {
        return text.to_string();
    }
    let keep: String = text.chars().take(max.saturating_sub(1)).collect();
    format!("{keep}\u{2026}")
}

fn text_width(text: &str, font: f64) -> f64 {
    text.chars().count() as f64 * font * 0.6
}

// ---------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------

/// The size of one rendering of the plot, in the SVG's own units.
#[derive(Debug, Clone, Copy)]
struct Geometry {
    width: f64,
    height: f64,
    /// Font sizes: tick labels and titles, and the smaller bucket names.
    font: f64,
    small: f64,
    /// A mark's radius.
    r: f64,
    /// The widest a categorical label on the vertical axis may be.
    max_y_label: f64,
}

/// For a wide container.
const WIDE: Geometry = Geometry {
    width: 860.0,
    height: 480.0,
    font: 12.0,
    small: 11.0,
    r: 5.5,
    max_y_label: 150.0,
};

/// For a phone-wide container (shown below 560 px by a container query).
const COMPACT: Geometry = Geometry {
    width: 330.0,
    height: 380.0,
    font: 11.5,
    small: 10.5,
    r: 6.0,
    max_y_label: 84.0,
};

/// Where an axis maps a case: along `p0..p1` (`p1 < p0` for the vertical).
struct Scale<'a> {
    axis: Option<&'a CaseAxis>,
    p0: f64,
    p1: f64,
    /// Numeric: kept clear at both ends, so a mark at the range's edge is not
    /// drawn on the frame.
    inset: f64,
}

impl Scale<'_> {
    fn len(&self) -> f64 {
        (self.p1 - self.p0).abs()
    }

    fn at(&self, t: f64) -> f64 {
        self.p0 + t * (self.p1 - self.p0)
    }

    fn numeric(&self) -> bool {
        self.axis.is_some_and(|a| a.kind == AxisKind::Numeric)
    }

    /// A number's position (numeric axes).
    fn value(&self, v: f64) -> f64 {
        let (lo, hi) = self.axis.map(CaseAxis::domain).unwrap_or((0.0, 1.0));
        let f = (self.inset / self.len()).min(0.25);
        self.at(f + (v - lo) / (hi - lo) * (1.0 - 2.0 * f))
    }

    fn bands(&self) -> usize {
        match self.axis {
            Some(a) if a.kind == AxisKind::Categorical => a.labels.len().max(1),
            _ => 1,
        }
    }

    /// The `i`th band's ends, as positions.
    fn band(&self, i: usize) -> (f64, f64) {
        let n = self.bands() as f64;
        (self.at(i as f64 / n), self.at((i as f64 + 1.0) / n))
    }

    /// Where a case goes, and the range it may be moved in.
    fn place(&self, case: &Case, salt: u64, r: f64) -> (f64, (f64, f64)) {
        let (lo, hi) = (self.p0.min(self.p1) + r, self.p0.max(self.p1) - r);
        match self.axis {
            Some(a) if a.kind == AxisKind::Numeric => {
                (self.value(case.number(&a.name).unwrap_or(0.0)), (lo, hi))
            }
            _ => {
                let i = self
                    .axis
                    .and_then(|a| {
                        let l = case.label(&a.name)?;
                        a.labels.iter().position(|x| *x == l)
                    })
                    .unwrap_or(0);
                let (b0, b1) = self.band(i);
                let (b0, b1) = (b0.min(b1), b0.max(b1));
                let width = b1 - b0;
                // Jitter inside the band, the same for the same case.
                let jit = (width * 0.3).min(26.0);
                let p = (b0 + b1) / 2.0 + (unit_hash(&case.key, salt) - 0.5) * 2.0 * jit;
                (p, ((b0 + r + 1.0).min(p), (b1 - r - 1.0).max(p)))
            }
        }
    }
}

/// A mark as laid out: where its value is, and where it is drawn.
#[derive(Debug, Clone, Copy)]
struct Mark {
    anchor: (f64, f64),
    at: (f64, f64),
}

impl Mark {
    fn moved(&self) -> bool {
        (self.at.0 - self.anchor.0).hypot(self.at.1 - self.anchor.1) > 2.5
    }
}

/// A `passes/runs` label as placed.
#[derive(Debug, Clone)]
struct Tag {
    case: usize,
    text: String,
    x: f64,
    y: f64,
    anchor: &'static str,
    /// A line from the mark to the label, when it had to go further out.
    leader: Option<(f64, f64, f64, f64)>,
}

type Rect = (f64, f64, f64, f64);

fn overlap(a: Rect, b: Rect) -> f64 {
    let w = (a.2.min(b.2) - a.0.max(b.0)).max(0.0);
    let h = (a.3.min(b.3) - a.1.max(b.1)).max(0.0);
    w * h
}

/// Push apart the marks closer than `gap`, inside their ranges.
/// The ranges a mark may be moved in: (x from, x to), (y from, y to).
type Range2 = ((f64, f64), (f64, f64));

fn dodge(at: &mut [(f64, f64)], ranges: &[Range2], gap: f64) {
    let n = at.len();
    for it in 0..150 {
        let mut grid: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
        for (i, p) in at.iter().enumerate() {
            grid.entry(((p.0 / gap).floor() as i64, (p.1 / gap).floor() as i64))
                .or_default()
                .push(i);
        }
        let mut delta = vec![(0.0f64, 0.0f64); n];
        let mut moved = false;
        for i in 0..n {
            let (cx, cy) = (
                (at[i].0 / gap).floor() as i64,
                (at[i].1 / gap).floor() as i64,
            );
            for gx in cx - 1..=cx + 1 {
                for gy in cy - 1..=cy + 1 {
                    let Some(list) = grid.get(&(gx, gy)) else {
                        continue;
                    };
                    for &j in list.iter().filter(|&&j| j > i) {
                        let (dx, dy) = (at[j].0 - at[i].0, at[j].1 - at[i].1);
                        let d = dx.hypot(dy);
                        if d >= gap - 1e-6 {
                            continue;
                        }
                        moved = true;
                        let (ux, uy) = if d < 1e-6 {
                            let a = (i as f64 * 0.618 + j as f64 * 0.314 + it as f64 * 0.17)
                                * std::f64::consts::TAU;
                            (a.cos(), a.sin())
                        } else {
                            (dx / d, dy / d)
                        };
                        let push = (gap - d) / 2.0 + 0.05;
                        delta[i].0 -= ux * push;
                        delta[i].1 -= uy * push;
                        delta[j].0 += ux * push;
                        delta[j].1 += uy * push;
                    }
                }
            }
        }
        if !moved {
            break;
        }
        for i in 0..n {
            let ((x0, x1), (y0, y1)) = ranges[i];
            at[i].0 = (at[i].0 + delta[i].0).clamp(x0, x1);
            at[i].1 = (at[i].1 + delta[i].1).clamp(y0, y1);
        }
    }
}

/// The plot laid out at one size.
struct Layout<'a> {
    g: Geometry,
    /// The plot area: left, top, right, bottom.
    area: Rect,
    x: Scale<'a>,
    y: Scale<'a>,
    marks: Vec<Mark>,
    tags: Vec<Tag>,
}

fn y_label_width(plot: &Plot, g: &Geometry) -> f64 {
    match &plot.y {
        Some(a) if a.kind == AxisKind::Categorical => a
            .labels
            .iter()
            .map(|l| text_width(l, g.font))
            .fold(0.0, f64::max)
            .min(g.max_y_label),
        Some(a) => {
            let (lo, hi) = a.domain();
            let ticks = y_ticks(a, lo, hi);
            ticks
                .iter()
                .map(|t| text_width(&short_number(*t), g.font))
                .fold(0.0, f64::max)
        }
        None => 0.0,
    }
}

/// Where a numeric axis has its tick labels: the bucket edges, or round
/// numbers when it has none.
fn y_ticks(a: &CaseAxis, lo: f64, hi: f64) -> Vec<f64> {
    let edges = a.edges();
    if edges.len() >= 2 {
        edges
    } else {
        nice_ticks(lo, hi, 5)
    }
}

fn layout<'a>(plot: &'a Plot, g: Geometry, rings: bool) -> Layout<'a> {
    let numeric_y = plot.y.as_ref().is_some_and(|a| a.kind == AxisKind::Numeric);
    // Left: the y labels, and (numeric) a column for its bucket names.
    let left = 10.0 + y_label_width(plot, &g) + if numeric_y { g.small + 10.0 } else { 0.0 };
    let numeric_x = plot.x.as_ref().is_some_and(|a| a.kind == AxisKind::Numeric);
    let bottom = g.font + 10.0 + if numeric_x { g.small + 8.0 } else { 0.0 } + g.font + 12.0;
    let area = (left.max(18.0), 30.0, g.width - 12.0, g.height - bottom);
    let x = Scale {
        axis: plot.x.as_ref(),
        p0: area.0,
        p1: area.2,
        inset: g.r + 6.0,
    };
    let y = Scale {
        axis: plot.y.as_ref(),
        p0: area.3,
        p1: area.1,
        inset: g.r + 6.0,
    };
    let mut anchors = Vec::with_capacity(plot.cases.len());
    let mut ranges = Vec::with_capacity(plot.cases.len());
    for c in &plot.cases {
        let (px, rx) = x.place(c, 1, g.r);
        let (py, ry) = y.place(c, 2, g.r);
        anchors.push((px, py));
        ranges.push((rx, ry));
    }
    let mut at = anchors.clone();
    // Apart by a mark and a bit, or by its dashed outline when there is one.
    let ring = if rings && plot.cases.iter().any(Case::for_instability) {
        3.5
    } else {
        0.0
    };
    dodge(&mut at, &ranges, 2.0 * (g.r + ring) + 2.0);
    let marks: Vec<Mark> = anchors
        .iter()
        .zip(&at)
        .map(|(a, p)| Mark { anchor: *a, at: *p })
        .collect();
    let tags = place_tags(plot, &marks, &g, area);
    Layout {
        g,
        area,
        x,
        y,
        marks,
        tags,
    }
}

/// Place each `passes/runs` label beside its mark, where it covers no mark
/// and no other label; further out, with a leader line, when nothing near
/// is free.
fn place_tags(plot: &Plot, marks: &[Mark], g: &Geometry, area: Rect) -> Vec<Tag> {
    let pad = g.r + 1.5;
    let mut taken: Vec<Rect> = marks
        .iter()
        .map(|m| (m.at.0 - pad, m.at.1 - pad, m.at.0 + pad, m.at.1 + pad))
        .collect();
    let bounds = (area.0 + 1.0, area.1 + 1.0, area.2 - 1.0, area.3 - 1.0);
    let mut order: Vec<usize> = (0..plot.cases.len())
        .filter(|&i| plot.cases[i].runs.len() > 1)
        .collect();
    order.sort_by_key(|&i| plot.cases[i].verdict().rank());
    let mut tags = Vec::new();
    for i in order {
        let c = &plot.cases[i];
        let text = format!("{}/{}", c.passes(), c.runs.len());
        let (w, h) = (text_width(&text, g.small) + 2.0, g.small * 1.15);
        let (mx, my) = marks[i].at;
        let mut best: Option<(f64, Tag, Rect)> = None;
        for (k, far) in [(0usize, 1.0f64), (1, 2.4), (2, 3.6)] {
            let d = (g.r + 3.0) * far;
            let dd = d * 0.75;
            // (left of the box, top of the box, anchor)
            let spots = [
                (mx + d, my - h / 2.0),
                (mx - d - w, my - h / 2.0),
                (mx - w / 2.0, my - d - h),
                (mx - w / 2.0, my + d),
                (mx + dd, my - dd - h),
                (mx + dd, my + dd),
                (mx - dd - w, my - dd - h),
                (mx - dd - w, my + dd),
            ];
            for (bx, by) in spots {
                let rect = (bx, by, bx + w, by + h);
                let mut cost: f64 = taken
                    .iter()
                    .enumerate()
                    .filter(|(j, _)| *j != i)
                    .map(|(_, t)| overlap(rect, *t))
                    .sum();
                // Outside the plot area counts as covering something.
                let inside = overlap(rect, bounds);
                cost += (w * h - inside) * 2.0;
                cost += k as f64 * 0.5;
                if best.as_ref().is_none_or(|b| cost < b.0) {
                    let leader = (k > 0).then(|| {
                        // From the mark's edge to the box's nearest point.
                        let (cx, cy) = (mx.clamp(bx, bx + w), my.clamp(by, by + h));
                        let (dx, dy) = (cx - mx, cy - my);
                        let dl = dx.hypot(dy).max(1e-6);
                        (
                            mx + dx / dl * (g.r + 1.5),
                            my + dy / dl * (g.r + 1.5),
                            cx,
                            cy,
                        )
                    });
                    best = Some((
                        cost,
                        Tag {
                            case: i,
                            text: text.clone(),
                            x: bx + w / 2.0,
                            y: by + h * 0.8,
                            anchor: "middle",
                            leader,
                        },
                        rect,
                    ));
                }
                if cost < 1e-9 {
                    break;
                }
            }
            if best.as_ref().is_some_and(|b| b.0 < 1.0 + k as f64 * 0.5) {
                break;
            }
        }
        if let Some((_, tag, rect)) = best {
            taken.push(rect);
            tags.push(tag);
        }
    }
    tags
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

fn axis_title(a: &CaseAxis) -> String {
    if a.unit.is_empty() {
        a.name.clone()
    } else {
        format!("{} ({})", a.name, a.unit)
    }
}

fn chosen_label(c: &str, labels: &Labels) -> String {
    match c {
        "coverage" => labels.chosen_coverage.clone(),
        "instability" => labels.chosen_instability.clone(),
        other => other.to_string(),
    }
}

fn verdict_label(v: Verdict, labels: &Labels) -> &str {
    match v {
        Verdict::Pass => &labels.state_pass,
        Verdict::Fail => &labels.state_fail,
        Verdict::Flaky => &labels.state_flaky,
    }
}

/// A mark's shape at the origin.
fn mark_shape(out: &mut String, v: Verdict, r: f64, instability: bool) {
    if instability {
        let _ = write!(out, "<circle class=\"oo-inst\" r=\"{:.1}\"/>", r + 3.5);
    }
    match v {
        Verdict::Pass => {
            let _ = write!(out, "<circle class=\"oo-m\" r=\"{r:.1}\"/>");
        }
        Verdict::Fail => {
            let a = r * 0.85;
            let _ = write!(
                out,
                "<path class=\"oo-m\" d=\"M{n:.1},{n:.1}L{a:.1},{a:.1}M{n:.1},{a:.1}L{a:.1},{n:.1}\"/>",
                n = -a
            );
        }
        Verdict::Flaky => {
            let _ = write!(out, "<circle class=\"oo-m\" r=\"{:.1}\"/>", r - 0.5);
        }
    }
}

/// The cards of the cases a component shows, once for all its panels:
/// each mark refers to its card by index (`data-oo-tip-i`), so the explorer
/// does not repeat them per pair of axes.
struct Tips {
    cards: Vec<serde_json::Value>,
    index: HashMap<String, usize>,
}

impl Tips {
    fn new(
        set: &CaseSet,
        query: &CaseQuery,
        labels: &Labels,
        predictor: Option<&dyn Predictor>,
    ) -> Self {
        let cases = set.selected(query);
        let scenarios = cases.iter().any(|c| c.scenario != cases[0].scenario);
        let mut index = HashMap::new();
        let cards = cases
            .iter()
            .enumerate()
            .map(|(i, c)| {
                index.insert(c.key.clone(), i);
                let mut card = case_tip(set, c, i + 1, scenarios, labels);
                if let Some(p) = predictor.and_then(|p| p.pass_probability(c)) {
                    if let Some(rows) = card["rows"].as_array_mut() {
                        rows.push(serde_json::json!([labels.model_pass, pct(p)]));
                    }
                }
                card
            })
            .collect();
        Tips { cards, index }
    }

    /// The cards as a JSON script element, safe inside HTML.
    fn script(&self) -> String {
        let json = serde_json::Value::Array(self.cards.clone())
            .to_string()
            .replace("</", "<\\/");
        format!("<script type=\"application/json\" class=\"oo-tips\">{json}</script>")
    }
}

/// A case's card: the rows the script shows.
fn case_tip(
    set: &CaseSet,
    case: &Case,
    n: usize,
    scenarios: bool,
    labels: &Labels,
) -> serde_json::Value {
    let axis = |a: &str| set.axes.get(a);
    let mut place = labels.case.replace("{n}", &n.to_string());
    if scenarios {
        let _ = write!(place, " \u{b7} {}", case.scenario);
    }
    let v = case.verdict();
    let state = format!(
        "{} \u{b7} {}/{}",
        verdict_label(v, labels),
        case.passes(),
        case.runs.len()
    );
    let mut rows: Vec<(String, String)> = Vec::new();
    // Every value drawn.
    for name in case.values.keys() {
        let mut text = value_of(case, axis(name), name, true);
        if let Some(b) = case.buckets.get(name).filter(|b| **b != text) {
            let _ = write!(text, "  {b}");
        }
        rows.push((name.clone(), text));
    }
    rows.push((
        labels.runs.clone(),
        labels
            .passes_of
            .replace("{p}", &case.passes().to_string())
            .replace("{n}", &case.runs.len().to_string()),
    ));
    const SHOWN: usize = 8;
    for r in case.runs.iter().take(SHOWN) {
        let what = if r.passed {
            labels.passed.clone()
        } else if let Some(t) = r.failed_at {
            labels.failed_at.replace("{t}", &long_number(t))
        } else {
            labels.failed.clone()
        };
        rows.push((
            labels.run.replace("{n}", &(r.document + 1).to_string()),
            what,
        ));
    }
    if case.runs.len() > SHOWN {
        rows.push((
            String::new(),
            labels
                .more_runs
                .replace("{n}", &(case.runs.len() - SHOWN).to_string()),
        ));
    }
    let chosen: Vec<String> = case
        .chosen_for()
        .iter()
        .filter(|c| !c.is_empty())
        .map(|c| chosen_label(c, labels))
        .collect();
    if !chosen.is_empty() {
        rows.push((labels.chosen_for.clone(), chosen.join(", ")));
    }
    if let Some(f) = case.flaky() {
        let num = |k: &str| f.get(k).and_then(serde_json::Value::as_f64);
        for (k, label, as_pct) in [
            ("pass_probability", &labels.pass_probability, true),
            ("flip_probability", &labels.flip_probability, true),
            (
                "instability_confidence",
                &labels.instability_confidence,
                true,
            ),
            ("robustness_radius", &labels.robustness_radius, false),
        ] {
            let text = match num(k) {
                Some(v) if as_pct => pct(v),
                Some(v) => long_number(v),
                None => "\u{2013}".into(),
            };
            rows.push((label.clone(), text));
        }
    }
    serde_json::json!({
        "place": place,
        "state": state,
        "kind": v.as_str(),
        "tentative": if case.for_instability() { Some(&labels.instability) } else { None },
        "rows": rows,
        "wide": true,
    })
}

/// The ticks, grid lines and names of one axis.
fn axis_svg(out: &mut String, lay: &Layout, horizontal: bool, labels: &Labels) {
    let g = &lay.g;
    let (l, t, r, b) = lay.area;
    let scale = if horizontal { &lay.x } else { &lay.y };
    let Some(a) = scale.axis else {
        return;
    };
    let _ = labels;
    if scale.numeric() {
        let (lo, hi) = a.domain();
        let edges = a.edges();
        // Grid lines at the bucket edges.
        for e in &edges {
            let p = scale.value(*e);
            if horizontal {
                let _ = write!(
                    out,
                    "<line class=\"oo-gl\" x1=\"{p:.1}\" y1=\"{t:.1}\" x2=\"{p:.1}\" y2=\"{b:.1}\"/>"
                );
            } else {
                let _ = write!(
                    out,
                    "<line class=\"oo-gl\" x1=\"{l:.1}\" y1=\"{p:.1}\" x2=\"{r:.1}\" y2=\"{p:.1}\"/>"
                );
            }
        }
        // Tick labels: the edges (or round numbers), never closer than
        // they can be read.
        let mut last: Option<f64> = None;
        for v in y_ticks(a, lo, hi) {
            let p = scale.value(v);
            let text = short_number(v);
            let room = if horizontal {
                text_width(&text, g.font) + 6.0
            } else {
                g.font + 2.0
            };
            if last.is_some_and(|q| (p - q).abs() < room) {
                continue;
            }
            last = Some(p);
            if horizontal {
                let _ = write!(
                    out,
                    "<line class=\"oo-tk\" x1=\"{p:.1}\" y1=\"{b:.1}\" x2=\"{p:.1}\" y2=\"{:.1}\"/>\
                     <text class=\"oo-tl\" x=\"{p:.1}\" y=\"{:.1}\" text-anchor=\"middle\">{}</text>",
                    b + 4.0,
                    b + 6.0 + g.font,
                    escape(&text)
                );
            } else {
                let _ = write!(
                    out,
                    "<line class=\"oo-tk\" x1=\"{:.1}\" y1=\"{p:.1}\" x2=\"{l:.1}\" y2=\"{p:.1}\"/>\
                     <text class=\"oo-tl\" x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"end\">{}</text>",
                    l - 4.0,
                    l - 6.0,
                    p + g.font * 0.35,
                    escape(&text)
                );
            }
        }
        // The buckets' names, in the middle of each, when they fit.
        for s in &a.spans {
            let (s0, s1) = (s.low.max(lo), s.high.min(hi));
            if s1 <= s0 {
                continue;
            }
            let (p0, p1) = (scale.value(s0), scale.value(s1));
            let room = (p1 - p0).abs() - 6.0;
            if text_width(&s.label, g.small) > room {
                continue;
            }
            let mid = (p0 + p1) / 2.0;
            if horizontal {
                let _ = write!(
                    out,
                    "<text class=\"oo-bl\" x=\"{mid:.1}\" y=\"{:.1}\" text-anchor=\"middle\">{}</text>",
                    b + 10.0 + g.font + g.small,
                    escape(&s.label)
                );
            } else {
                let x = l - 8.0 - y_label_width_numeric(a, g) - g.small * 0.3;
                let _ = write!(
                    out,
                    "<text class=\"oo-bl\" x=\"{x:.1}\" y=\"{mid:.1}\" text-anchor=\"middle\" \
                     transform=\"rotate(-90 {x:.1} {mid:.1})\">{}</text>",
                    escape(&s.label)
                );
            }
        }
    } else {
        let n = scale.bands();
        for i in 0..n {
            let (p0, p1) = scale.band(i);
            if i > 0 {
                if horizontal {
                    let _ = write!(
                        out,
                        "<line class=\"oo-gl\" x1=\"{p0:.1}\" y1=\"{t:.1}\" x2=\"{p0:.1}\" y2=\"{b:.1}\"/>"
                    );
                } else {
                    let _ = write!(
                        out,
                        "<line class=\"oo-gl\" x1=\"{l:.1}\" y1=\"{p0:.1}\" x2=\"{r:.1}\" y2=\"{p0:.1}\"/>"
                    );
                }
            }
            let Some(label) = a.labels.get(i) else {
                continue;
            };
            let mid = (p0 + p1) / 2.0;
            if horizontal {
                let text = fit(label, (p1 - p0).abs() - 4.0, g.font);
                let _ = write!(
                    out,
                    "<text class=\"oo-tl\" x=\"{mid:.1}\" y=\"{:.1}\" text-anchor=\"middle\"><title>{}</title>{}</text>",
                    b + 6.0 + g.font,
                    escape(label),
                    escape(&text)
                );
            } else {
                let text = fit(label, g.max_y_label, g.font);
                let _ = write!(
                    out,
                    "<text class=\"oo-tl\" x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"end\"><title>{}</title>{}</text>",
                    l - 6.0,
                    mid + g.font * 0.35,
                    escape(label),
                    escape(&text)
                );
            }
        }
    }
    // The axis' name: under the plot (x), above it (y).
    let title = axis_title(a);
    if horizontal {
        let y = g.height - 6.0;
        let _ = write!(
            out,
            "<text class=\"oo-atitle\" x=\"{:.1}\" y=\"{y:.1}\" text-anchor=\"middle\"><title>{}</title>{} \u{2192}</text>",
            (l + r) / 2.0,
            escape(&a.text),
            escape(&fit(&title, g.width - 24.0, g.font))
        );
    } else {
        let _ = write!(
            out,
            "<text class=\"oo-atitle\" x=\"6\" y=\"{:.1}\" text-anchor=\"start\"><title>{}</title>\u{2191} {}</text>",
            t - 12.0,
            escape(&a.text),
            escape(&fit(&title, g.width - 24.0, g.font))
        );
    }
}

/// The positions a grid axis' cells span on `scale`, by the grid's cell
/// order; `None` for a cell the plot has no band for.
fn grid_spans(scale: &Scale, axis: &GridAxis, frame: (f64, f64)) -> Vec<Option<(f64, f64)>> {
    match axis {
        GridAxis::Numeric { edges, .. } => {
            let n = edges.len().saturating_sub(1);
            (0..n)
                .map(|i| {
                    // The outer cells reach the frame.
                    let a = if i == 0 {
                        frame.0
                    } else {
                        scale.value(edges[i])
                    };
                    let b = if i + 1 == n {
                        frame.1
                    } else {
                        scale.value(edges[i + 1])
                    };
                    Some((a.min(b), a.max(b)))
                })
                .collect()
        }
        GridAxis::Categorical { labels, .. } => labels
            .iter()
            .map(|l| {
                let i = match scale.axis {
                    Some(a) if a.kind == AxisKind::Categorical => {
                        a.labels.iter().position(|x| x == l)?
                    }
                    None => 0,
                    _ => return None,
                };
                let (a, b) = scale.band(i);
                Some((a.min(b), a.max(b)))
            })
            .collect(),
    }
}

/// The predicted states, as faint rectangles behind everything else: each
/// row's runs of one state as one rectangle.
fn background(out: &mut String, lay: &Layout, grid: &Grid) {
    let (l, t, r, b) = lay.area;
    if lay.x.axis.map(|a| a.name.as_str()) != Some(grid.x.name())
        || lay.y.axis.map(|a| a.name.as_str()) != Some(grid.y.name())
    {
        return;
    }
    let xs = grid_spans(&lay.x, &grid.x, (l, r));
    let ys = grid_spans(&lay.y, &grid.y, (b, t));
    out.push_str("<g class=\"oo-pred\" aria-hidden=\"true\">");
    for (row, cells) in grid.cells.iter().enumerate() {
        let Some(Some((y0, y1))) = ys.get(row) else {
            continue;
        };
        let mut c = 0;
        while c < cells.len() {
            let (Some(cell), Some(Some((x0, _)))) = (cells[c], xs.get(c)) else {
                c += 1;
                continue;
            };
            let mut end = c;
            while end + 1 < cells.len()
                && cells[end + 1].is_some_and(|n| n.state == cell.state)
                && xs.get(end + 1).is_some_and(|s| s.is_some())
            {
                end += 1;
            }
            let x1 = xs[end].map(|s| s.1).unwrap_or(*x0);
            let _ = write!(
                out,
                "<rect class=\"oo-pred-{}\" x=\"{x0:.1}\" y=\"{y0:.1}\" width=\"{:.1}\" height=\"{:.1}\"/>",
                cell.state.as_str(),
                (x1 - x0).max(0.0) + 0.4,
                (y1 - y0).max(0.0) + 0.4
            );
            c = end + 1;
        }
    }
    out.push_str("</g>");
}

fn y_label_width_numeric(a: &CaseAxis, g: &Geometry) -> f64 {
    let (lo, hi) = a.domain();
    y_ticks(a, lo, hi)
        .iter()
        .map(|t| text_width(&short_number(*t), g.font))
        .fold(0.0, f64::max)
}

#[allow(clippy::too_many_arguments)]
fn svg(
    out: &mut String,
    plot: &Plot,
    lay: &Layout,
    class: &str,
    tips: &Tips,
    grid: Option<&Grid>,
    opts: &Options,
) {
    let g = &lay.g;
    let labels = &opts.labels;
    let (l, t, r, b) = lay.area;
    let name = match (&plot.x, &plot.y) {
        (Some(x), Some(y)) => format!("{}: {} \u{d7} {}", labels.title, x.name, y.name),
        (Some(x), None) => format!("{}: {}", labels.title, x.name),
        _ => labels.title.clone(),
    };
    let _ = write!(
        out,
        "<svg class=\"oo-svg {class}\" viewBox=\"0 0 {} {}\" role=\"group\" aria-label=\"{}\">",
        g.width,
        g.height,
        escape(&name)
    );
    let _ = write!(
        out,
        "<rect class=\"oo-frame\" x=\"{l:.1}\" y=\"{t:.1}\" width=\"{:.1}\" height=\"{:.1}\" rx=\"6\"/>",
        r - l,
        b - t
    );
    if let Some(grid) = grid {
        background(out, lay, grid);
    }
    axis_svg(out, lay, true, labels);
    axis_svg(out, lay, false, labels);
    // Leader lines from a moved mark to its value.
    for m in lay.marks.iter().filter(|m| m.moved()) {
        let _ = write!(
            out,
            "<line class=\"oo-lead\" x1=\"{:.1}\" y1=\"{:.1}\" x2=\"{:.1}\" y2=\"{:.1}\"/>\
             <circle class=\"oo-dot\" cx=\"{:.1}\" cy=\"{:.1}\" r=\"1.6\"/>",
            m.anchor.0, m.anchor.1, m.at.0, m.at.1, m.anchor.0, m.anchor.1
        );
    }
    for tag in &lay.tags {
        if let Some((x1, y1, x2, y2)) = tag.leader {
            let _ = write!(
                out,
                "<line class=\"oo-lead oo-lead-tag\" x1=\"{x1:.1}\" y1=\"{y1:.1}\" x2=\"{x2:.1}\" y2=\"{y2:.1}\"/>"
            );
        }
    }
    // The marks: passes first, so failures and flaky cases are drawn on top.
    let mut order: Vec<usize> = (0..plot.cases.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(plot.cases[i].verdict().rank()));
    for i in order {
        let c = &plot.cases[i];
        let m = &lay.marks[i];
        let v = c.verdict();
        let mut text = [&plot.x, &plot.y]
            .into_iter()
            .flatten()
            .map(|a| format!("{}={}", a.name, value_of(c, Some(a), &a.name, false)))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = write!(
            text,
            ": {} {}/{}",
            verdict_label(v, labels),
            c.passes(),
            c.runs.len()
        );
        if c.for_instability() {
            let _ = write!(text, " ({})", labels.instability);
        }
        let rings = opts.instability_rings && c.for_instability();
        let tip = tips.index.get(&c.key).copied().unwrap_or(0);
        let _ = write!(
            out,
            "<g class=\"oo-case oo-{}{}\" transform=\"translate({:.1} {:.1})\" tabindex=\"0\" role=\"img\" \
             aria-label=\"{}\" data-oo-tip-i=\"{tip}\"><circle class=\"oo-hit\" r=\"{:.1}\"/>",
            v.as_str(),
            if rings { " oo-instab" } else { "" },
            m.at.0,
            m.at.1,
            escape(&text),
            g.r + 5.0
        );
        mark_shape(out, v, g.r, rings);
        out.push_str("</g>");
    }
    for tag in &lay.tags {
        let v = plot.cases[tag.case].verdict();
        let _ = write!(
            out,
            "<text class=\"oo-nm oo-nm-{}\" x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"{}\" aria-hidden=\"true\">{}</text>",
            v.as_str(),
            tag.x,
            tag.y,
            tag.anchor,
            escape(&tag.text)
        );
    }
    out.push_str("</svg>");
}

/// A mark as a legend key.
fn key(v: Verdict, instability: bool) -> String {
    let mut s = String::from(
        "<svg class=\"oo-key\" viewBox=\"-9 -9 18 18\" width=\"18\" height=\"18\" aria-hidden=\"true\"><g class=\"oo-case oo-",
    );
    s.push_str(v.as_str());
    if instability {
        s.push_str(" oo-instab");
    }
    s.push_str("\">");
    mark_shape(&mut s, v, if instability { 3.5 } else { 5.0 }, instability);
    s.push_str("</g></svg>");
    s
}

/// How a panel's axis pickers act.
enum AxesUi<'a> {
    /// Host-driven: dispatch `openodd:axes` (and follow the link, if any).
    Host,
    /// Self-contained: show the panel of the pair picked, `x\u{1f}y` -> id.
    Explorer(&'a [(String, String)]),
}

fn pickers(out: &mut String, plot: &Plot, free: &[String], ui: &AxesUi, opts: &Options) {
    let (Some(x), Some(y)) = (&plot.x, &plot.y) else {
        return;
    };
    if free.len() < 2 {
        return;
    }
    let labels = &opts.labels;
    let mut attrs = String::from(" data-oo-conditions=\"[]\" data-oo-view=\"cases\"");
    match ui {
        AxesUi::Host => {
            if let Some(link) = &opts.link {
                let _ = write!(attrs, " data-oo-link=\"{}\" data-oo-at=\"\"", escape(link));
            }
        }
        AxesUi::Explorer(map) => {
            let map: serde_json::Map<String, serde_json::Value> = map
                .iter()
                .map(|(k, v)| (k.clone(), serde_json::Value::from(v.as_str())))
                .collect();
            let _ = write!(
                attrs,
                " data-oo-map=\"{}\"",
                escape(&serde_json::Value::Object(map).to_string())
            );
        }
    }
    let _ = write!(out, "<div class=\"oo-axes oo-case-axes\"{attrs}>");
    for (axis, title, arrow, current) in [
        ("x", &labels.x, "\u{2192}", &x.name),
        ("y", &labels.y, "\u{2191}", &y.name),
    ] {
        let _ = write!(
            out,
            "<label class=\"oo-pick oo-pick-{axis}\"><span class=\"oo-cap\">{} <span aria-hidden=\"true\">{arrow}</span></span>\
             <select data-oo-axis=\"{axis}\" data-oo-current=\"{}\" aria-label=\"{}\">",
            escape(title),
            escape(current),
            escape(title)
        );
        for a in free {
            let _ = write!(
                out,
                "<option value=\"{}\"{}>{}</option>",
                escape(a),
                if a == current { " selected" } else { "" },
                escape(a)
            );
        }
        out.push_str("</select></label>");
    }
    out.push_str("</div>");
}

/// The table of the cases run more than once, without the root element.
fn table_inner(out: &mut String, plot: &Plot, opts: &Options) {
    let labels = &opts.labels;
    let _ = write!(
        out,
        "<div class=\"oo-case-table\"><h4>{}</h4>",
        escape(&labels.table_title)
    );
    let repeated = plot.repeated();
    if repeated.is_empty() {
        let _ = write!(
            out,
            "<p class=\"oo-note\">{}</p></div>",
            escape(&labels.no_repeated)
        );
        return;
    }
    // The plotted attributes first, then the rest the cases have.
    let mut names: Vec<String> = [&plot.x, &plot.y]
        .into_iter()
        .flatten()
        .map(|a| a.name.clone())
        .collect();
    for c in &repeated {
        for k in c.values.keys() {
            if !names.contains(k) {
                names.push(k.clone());
            }
        }
    }
    let plotted = [&plot.x, &plot.y].into_iter().flatten().count();
    let _ = write!(
        out,
        "<div class=\"oo-scroll oo-table-scroll\"><table class=\"oo-table\"><thead><tr><th scope=\"col\">{}</th>",
        escape(&labels.verdict)
    );
    for (i, n) in names.iter().enumerate() {
        let unit = [&plot.x, &plot.y]
            .into_iter()
            .flatten()
            .find(|a| &a.name == n)
            .map(|a| a.unit.clone())
            .unwrap_or_default();
        let _ = write!(
            out,
            "<th scope=\"col\" class=\"{}\">{}{}</th>",
            if i < plotted { "oo-plotted" } else { "" },
            escape(n),
            if unit.is_empty() {
                String::new()
            } else {
                format!(" <span class=\"oo-unit\">{}</span>", escape(&unit))
            }
        );
    }
    let _ = write!(
        out,
        "<th scope=\"col\">{}</th><th scope=\"col\">{}</th><th scope=\"col\">{}</th></tr></thead><tbody>",
        escape(&labels.runs),
        escape(&labels.outcomes),
        escape(&labels.chosen_for)
    );
    for c in repeated {
        let v = c.verdict();
        let _ = write!(
            out,
            "<tr class=\"oo-row-{}\"><th scope=\"row\" class=\"oo-v-cell\"><span class=\"oo-verdict\">{}{}</span></th>",
            v.as_str(),
            key(v, false),
            escape(verdict_label(v, labels))
        );
        for (i, n) in names.iter().enumerate() {
            let axis = [&plot.x, &plot.y]
                .into_iter()
                .flatten()
                .find(|a| &a.name == n);
            let _ = write!(
                out,
                "<td class=\"oo-val{}\">{}</td>",
                if i < plotted { " oo-plotted" } else { "" },
                escape(&value_of(c, axis, n, false))
            );
        }
        let _ = write!(
            out,
            "<td class=\"oo-val\">{}/{}</td><td><span class=\"oo-seq\">",
            c.passes(),
            c.runs.len()
        );
        for r in &c.runs {
            let (cls, letter, word) = if r.passed {
                ("pass", &labels.pass_short, &labels.passed)
            } else {
                ("fail", &labels.fail_short, &labels.failed)
            };
            let _ = write!(
                out,
                "<b class=\"oo-o oo-o-{cls}\" title=\"{}: {}\">{}</b>",
                escape(&labels.run.replace("{n}", &(r.document + 1).to_string())),
                escape(word),
                escape(letter)
            );
        }
        let chosen: Vec<String> = c
            .chosen_for()
            .iter()
            .filter(|c| !c.is_empty())
            .map(|c| chosen_label(c, labels))
            .collect();
        let _ = write!(
            out,
            "</span></td><td>{}</td></tr>",
            escape(&chosen.join(", "))
        );
    }
    out.push_str("</tbody></table></div></div>");
}

#[allow(clippy::too_many_arguments)]
fn panel(
    out: &mut String,
    plot: &Plot,
    free: &[String],
    ui: &AxesUi,
    tips: &Tips,
    predictor: Option<&dyn Predictor>,
    opts: &Options,
    panel_id: &str,
    hidden: bool,
) {
    let labels = &opts.labels;
    let _ = write!(
        out,
        "<section class=\"oo-panel\" id=\"{}\" tabindex=\"-1\"{}>",
        escape(panel_id),
        if hidden { " hidden" } else { "" }
    );
    let counts = plot.counts();
    let total = plot.cases.len();
    let repeated = plot.cases.iter().filter(|c| c.runs.len() > 1).count();
    let stat = |out: &mut String, k: &str, v: String, sub: String, kind: &str| {
        let _ = write!(
            out,
            "<div class=\"oo-stat oo-stat-{kind}\"><span class=\"oo-k\">{}</span>\
             <span class=\"oo-v\">{v}</span><span class=\"oo-s\">{}</span></div>",
            escape(k),
            escape(&sub)
        );
    };
    let of_cases = labels.of_cases.replace("{n}", &total.to_string());
    out.push_str("<div class=\"oo-stats\">");
    stat(
        out,
        &labels.cases,
        total.to_string(),
        labels.runs_of.replace("{n}", &plot.runs().to_string()),
        "runs",
    );
    stat(
        out,
        &labels.repeated,
        repeated.to_string(),
        labels.repeated_sub.clone(),
        "repeated",
    );
    stat(
        out,
        &labels.flaky_cases,
        counts["flaky"].to_string(),
        of_cases.clone(),
        "flaky",
    );
    stat(
        out,
        &labels.failing_cases,
        counts["fail"].to_string(),
        of_cases,
        "fail",
    );
    out.push_str("</div>");
    if plot.environments.len() > 1 {
        let _ = write!(
            out,
            "<p class=\"oo-warn\">{}</p>",
            escape(&labels.pooled_environments)
        );
    }
    let (Some(_), true) = (&plot.x, total > 0) else {
        let _ = write!(
            out,
            "<p class=\"oo-note\">{}</p>",
            escape(&labels.no_attributes)
        );
        notes(out, plot, labels);
        out.push_str("</section>");
        return;
    };
    let rings = opts.instability_rings;
    let wide = layout(plot, WIDE, rings);
    let compact = layout(plot, COMPACT, rings);
    let grid = predictor.map(|p| p.grid(plot));
    let shown = match &grid {
        Some(Ok(g)) => Some(g),
        _ => None,
    };
    // The legend: each mark, with its count.
    out.push_str("<div class=\"oo-legend oo-case-legend\">");
    for v in [Verdict::Pass, Verdict::Fail, Verdict::Flaky] {
        let _ = write!(
            out,
            "<span class=\"oo-chip\">{}{}<b>{}</b></span>",
            key(v, false),
            escape(verdict_label(v, labels)),
            counts[v.as_str()]
        );
    }
    let instab = plot.cases.iter().filter(|c| c.for_instability()).count();
    if rings && instab > 0 {
        let _ = write!(
            out,
            "<span class=\"oo-chip\">{}{}<b>{instab}</b></span>",
            key(Verdict::Pass, true),
            escape(&labels.instability)
        );
    }
    if repeated > 0 {
        let _ = write!(
            out,
            "<span class=\"oo-chip\"><i class=\"oo-nm-key\">n/m</i>{}</span>",
            escape(&labels.ratio)
        );
    }
    if let Some(g) = shown {
        for (state, text) in [
            (Verdict::Pass, &labels.predicted_pass),
            (Verdict::Fail, &labels.predicted_fail),
            (Verdict::Flaky, &labels.predicted_flaky),
        ] {
            if g.cells.iter().flatten().flatten().any(|c| c.state == state) {
                let _ = write!(
                    out,
                    "<span class=\"oo-chip\"><i class=\"oo-pred-key oo-pred-{}\"></i>{}</span>",
                    state.as_str(),
                    escape(text)
                );
            }
        }
    }
    if wide.marks.iter().chain(&compact.marks).any(Mark::moved) {
        let _ = write!(
            out,
            "<span class=\"oo-chip\"><svg class=\"oo-key\" viewBox=\"0 0 18 18\" width=\"18\" height=\"18\" aria-hidden=\"true\">\
             <line class=\"oo-lead\" x1=\"3\" y1=\"14\" x2=\"12\" y2=\"6\"/><circle class=\"oo-dot\" cx=\"3\" cy=\"14\" r=\"1.6\"/></svg>{}</span>",
            escape(&labels.moved)
        );
    }
    out.push_str("</div>");
    pickers(out, plot, free, ui, opts);
    out.push_str("<div class=\"oo-plot\">");
    svg(out, plot, &wide, "oo-plot-wide", tips, shown, opts);
    svg(out, plot, &compact, "oo-plot-compact", tips, shown, opts);
    out.push_str("</div>");
    match &grid {
        Some(Ok(g)) => {
            let pct0 = |x: f64| format!("{}%", short_number(x * 100.0));
            let _ = write!(
                out,
                "<p class=\"oo-note oo-pred-note\">{}",
                escape(
                    &labels
                        .predicted_caption
                        .replace("{runs}", &g.runs.to_string())
                        .replace("{confidence}", &pct0(g.flaky_confidence))
                        .replace("{threshold}", &pct0(g.flip_threshold))
                )
            );
            if !g.held.is_empty() {
                let held: Vec<String> = g.held.iter().map(|(a, v)| format!("{a} = {v}")).collect();
                let _ = write!(
                    out,
                    " {}",
                    escape(&labels.held.replace("{held}", &held.join(", ")))
                );
            }
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
    if opts.table {
        table_inner(out, plot, opts);
    }
    notes(out, plot, labels);
    out.push_str("</section>");
}

fn notes(out: &mut String, plot: &Plot, labels: &Labels) {
    if plot.unplaced > 0 {
        let _ = write!(
            out,
            "<p class=\"oo-note\">{} {}.</p>",
            plot.unplaced,
            escape(&labels.unplaced)
        );
    }
    let skipped = plot.without_case + plot.without_outcome;
    if skipped > 0 {
        let _ = write!(
            out,
            "<p class=\"oo-note\">{skipped} {}.</p>",
            escape(&labels.skipped)
        );
    }
}

fn open_root(out: &mut String, opts: &Options, mode: &str) {
    let _ = write!(
        out,
        "<div class=\"oo-cov oo-cases\" id=\"{}\" data-oo-mode=\"{mode}\"><div class=\"oo-head\"><h3>{}</h3><p class=\"oo-hint\">{}</p></div>",
        escape(opts.root_id()),
        escape(&opts.labels.title),
        escape(&opts.labels.hint)
    );
}

/// One plot, host-driven: the axis pickers dispatch `openodd:axes` (and
/// follow [`Options::link`] if given); the host renders the plot asked for.
/// Followed by the table of repeated cases unless [`Options::table`] is off.
pub fn fragment(set: &CaseSet, plot: &Plot, opts: &Options) -> String {
    fragment_with(set, plot, opts, None)
}

/// [`fragment`], with the background `predictor` gives (see
/// [`crate::prediction`]), and its pass probability in each case's card.
pub fn fragment_with(
    set: &CaseSet,
    plot: &Plot,
    opts: &Options,
    predictor: Option<&dyn Predictor>,
) -> String {
    let mut out = String::new();
    open_root(&mut out, opts, "host");
    let tips = Tips::new(set, &plot.query, &opts.labels, predictor);
    let id = format!("{}-p0", opts.root_id());
    panel(
        &mut out,
        plot,
        &plot.attributes,
        &AxesUi::Host,
        &tips,
        predictor,
        opts,
        &id,
        false,
    );
    out.push_str(&tips.script());
    out.push_str("</div>");
    out
}

/// The table of the cases run more than once, on its own: each one's values
/// (the plotted attributes first), its outcomes in order and its verdict.
pub fn table(plot: &Plot, opts: &Options) -> String {
    let mut out = format!(
        "<div class=\"oo-cov oo-cases oo-cases-table\" id=\"{}\">",
        escape(opts.root_id())
    );
    table_inner(&mut out, plot, opts);
    if plot.environments.len() > 1 {
        let _ = write!(
            out,
            "<p class=\"oo-warn\">{}</p>",
            escape(&opts.labels.pooled_environments)
        );
    }
    out.push_str("</div>");
    out
}

/// The separator of a pair in the explorer's map of axis pairs.
const PAIR_SEP: char = '\u{1f}';

/// The plot `query` asks for and, as hidden panels the pickers show, one for
/// every other pair of attributes (at most [`MAX_AXIS_PAIRS`]): nothing to
/// serve.
pub fn explorer(set: &CaseSet, query: &CaseQuery, opts: &Options) -> String {
    explorer_with(set, query, opts, None)
}

/// [`explorer`], each pair with the background `predictor` gives.
pub fn explorer_with(
    set: &CaseSet,
    query: &CaseQuery,
    opts: &Options,
    predictor: Option<&dyn Predictor>,
) -> String {
    let mut out = String::new();
    open_root(&mut out, opts, "explorer");
    let root = opts.root_id().to_string();
    let first = set.plot(query);
    let tips = Tips::new(set, query, &opts.labels, predictor);
    let mut free = first.attributes.clone();
    while free.len() > 2 && free.len() * (free.len() - 1) > MAX_AXIS_PAIRS {
        free.pop();
    }
    let name = |a: &Option<CaseAxis>| a.as_ref().map(|a| a.name.clone()).unwrap_or_default();
    let asked = (name(&first.x), name(&first.y));
    // The pickers offer the asked pair's attributes even past the cap.
    for a in [&asked.0, &asked.1] {
        if !a.is_empty() && !free.contains(a) {
            free.push(a.clone());
        }
    }
    let mut pairs: Vec<(String, String)> = vec![asked.clone()];
    for x in &free {
        for y in &free {
            let pair = (x.clone(), y.clone());
            if x != y && pair != asked && pairs.len() < MAX_AXIS_PAIRS {
                pairs.push(pair);
            }
        }
    }
    let map: Vec<(String, String)> = pairs
        .iter()
        .enumerate()
        .map(|(i, (x, y))| (format!("{x}{PAIR_SEP}{y}"), format!("{root}-p{i}")))
        .collect();
    for (i, (x, y)) in pairs.iter().enumerate() {
        let plot = if i == 0 {
            first.clone()
        } else {
            set.plot(&CaseQuery {
                x: Some(x.clone()),
                y: Some(y.clone()),
                ..query.clone()
            })
        };
        panel(
            &mut out,
            &plot,
            &free,
            &AxesUi::Explorer(&map),
            &tips,
            predictor,
            opts,
            &map[i].1,
            i > 0,
        );
    }
    out.push_str(&tips.script());
    out.push_str("</div>");
    out
}

/// A standalone page: the styles, the script and `body` (as
/// [`crate::html::page`]).
pub fn page(title: &str, body: &str) -> String {
    crate::html::page(title, body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_read_well() {
        assert_eq!(short_number(13.04), "13");
        assert_eq!(short_number(13.2), "13.2");
        assert_eq!(short_number(-0.0001), "0");
        assert_eq!(short_number(3.456), "3.46");
        assert_eq!(short_number(1234.6), "1235");
        assert_eq!(long_number(20.390184), "20.39");
    }

    #[test]
    fn nice_ticks_are_round() {
        assert_eq!(
            nice_ticks(0.0, 10.0, 5),
            vec![0.0, 2.0, 4.0, 6.0, 8.0, 10.0]
        );
        assert_eq!(
            nice_ticks(-15.0, 5.0, 4),
            vec![-15.0, -10.0, -5.0, 0.0, 5.0]
        );
    }

    #[test]
    fn dodging_separates_coincident_marks() {
        let mut at = vec![(50.0, 50.0), (50.0, 50.0), (50.5, 50.0), (200.0, 50.0)];
        let ranges = vec![((0.0, 300.0), (0.0, 300.0)); 4];
        dodge(&mut at, &ranges, 10.0);
        for i in 0..at.len() {
            for j in i + 1..at.len() {
                let d = (at[i].0 - at[j].0).hypot(at[i].1 - at[j].1);
                assert!(d >= 9.9, "{i} {j} {d}");
            }
        }
        assert_eq!(at[3], (200.0, 50.0));
    }

    #[test]
    fn labels_set_by_name() {
        let mut l = Labels::default();
        assert!(l.set("title", "ケース"));
        assert_eq!(l.title, "ケース");
        assert!(!l.set("nope", "x"));
    }
}
