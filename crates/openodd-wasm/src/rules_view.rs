//! A backend's rules (conditional distributions), as a component: for one
//! quantity at the inputs chosen, what each part and the quantity give there
//! (the range, the median, the mean and the value at a quantile `p`, the part
//! deciding it marked), the quantity's distribution there, how the quantity
//! answers one input moved across its range, and each part over one of its
//! inputs with the points written.
//!
//! [`html`] renders a fragment (`<div class="oo-cov oo-rules">`) on the
//! coverage view's styles and theme variables, plus [`styles`] and
//! [`script`] of its own; the script dispatches `openodd:rules` from its
//! sliders and pickers, which `js/viewer.js` (`mount`, `view: "rules"`)
//! answers by rendering again. [`data`] is the same as JSON, for a host that
//! draws it its own way.

use indexmap::IndexMap;
use openodd::rules::{Dist, Joined, Part, Quantity, RuleSet};
use openodd_coverage_view::html::escape;
use serde_json::{json, Value as Json};

/// Which quantity to show, at which inputs and quantile, moving which input.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Query {
    /// The quantity (the first, if `None`).
    pub quantity: Option<String>,
    /// Input -> its value; one not given reads its default, else the middle
    /// of its range.
    pub inputs: IndexMap<String, f64>,
    /// The input the response chart moves (the quantity's first, if `None`).
    pub x: Option<String>,
    /// The quantile shown, 0 to 1 (0.5, if `None`).
    pub p: Option<f64>,
}

impl Query {
    /// From JSON: `{quantity, inputs: {name: value}, x, p}`.
    pub fn from_json(raw: &Json) -> Result<Self, String> {
        let mut q = Query {
            quantity: raw.get("quantity").and_then(Json::as_str).map(String::from),
            x: raw.get("x").and_then(Json::as_str).map(String::from),
            ..Default::default()
        };
        if let Some(p) = raw.get("p").filter(|p| !p.is_null()) {
            let p = p.as_f64().ok_or("p: not a number")?;
            if !(0.0..=1.0).contains(&p) {
                return Err("p: from 0 to 1".into());
            }
            q.p = Some(p);
        }
        if let Some(map) = raw.get("inputs").and_then(Json::as_object) {
            for (k, v) in map {
                let v = v
                    .as_f64()
                    .ok_or_else(|| format!("inputs.{k}: not a number"))?;
                q.inputs.insert(k.clone(), v);
            }
        }
        Ok(q)
    }
}

/// The component's words; `set` replaces one by its field name.
#[derive(Debug, Clone, PartialEq)]
pub struct Labels {
    pub inputs: String,
    pub outputs: String,
    pub quantity: String,
    pub response: String,
    pub vary: String,
    pub distribution: String,
    pub parts: String,
    pub part: String,
    pub combine: String,
    pub quantile: String,
    pub low: String,
    pub high: String,
    pub median: String,
    pub mean: String,
    pub at_p: String,
    pub deciding: String,
    pub points: String,
}

impl Default for Labels {
    fn default() -> Self {
        Labels {
            inputs: "Inputs".into(),
            outputs: "Outputs".into(),
            quantity: "Quantity".into(),
            response: "Response".into(),
            vary: "Vary".into(),
            distribution: "Distribution".into(),
            parts: "parts".into(),
            part: "part".into(),
            combine: "combine".into(),
            quantile: "quantile p".into(),
            low: "low".into(),
            high: "high".into(),
            median: "median".into(),
            mean: "mean".into(),
            at_p: "at p".into(),
            deciding: "decides".into(),
            points: "points written".into(),
        }
    }
}

impl Labels {
    /// Set one label by its field name; `false` if there is no such label.
    pub fn set(&mut self, name: &str, value: &str) -> bool {
        let slot = match name {
            "inputs" => &mut self.inputs,
            "outputs" => &mut self.outputs,
            "quantity" => &mut self.quantity,
            "response" => &mut self.response,
            "vary" => &mut self.vary,
            "distribution" => &mut self.distribution,
            "parts" => &mut self.parts,
            "part" => &mut self.part,
            "combine" => &mut self.combine,
            "quantile" => &mut self.quantile,
            "low" => &mut self.low,
            "high" => &mut self.high,
            "median" => &mut self.median,
            "mean" => &mut self.mean,
            "at_p" => &mut self.at_p,
            "deciding" => &mut self.deciding,
            "points" => &mut self.points,
            _ => return false,
        };
        *slot = value.to_string();
        true
    }
}

/// The component's options.
#[derive(Debug, Clone, PartialEq)]
pub struct Options {
    /// The root element's id.
    pub id: String,
    pub labels: Labels,
    /// Points per curve.
    pub samples: usize,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            id: String::new(),
            labels: Labels::default(),
            samples: 121,
        }
    }
}

impl Options {
    /// From JSON: `{id, labels, samples}`.
    pub fn from_json(raw: &Json) -> Result<Self, String> {
        let mut out = Options {
            id: raw
                .get("id")
                .and_then(Json::as_str)
                .unwrap_or_default()
                .to_string(),
            ..Default::default()
        };
        if let Some(n) = raw.get("samples") {
            let n = n.as_u64().ok_or("samples: not a whole number")? as usize;
            if !(2..=2001).contains(&n) {
                return Err("samples: from 2 to 2001".into());
            }
            out.samples = n;
        }
        if let Some(map) = raw.get("labels").and_then(Json::as_object) {
            for (k, v) in map {
                let v = v.as_str().ok_or(format!("label {k}: not a string"))?;
                if !out.labels.set(k, v) {
                    return Err(format!("no such label: {k}"));
                }
            }
        }
        Ok(out)
    }
}

/// An input of a quantity: its range (the outermost points of every part
/// reading it), its default, and the value chosen.
#[derive(Debug, Clone, PartialEq)]
pub struct Slider {
    pub name: String,
    pub low: f64,
    pub high: f64,
    pub default: Option<f64>,
    pub value: f64,
}

/// A quantity's inputs, each with its range and default; `value` is the
/// default, else the middle of the range.
pub fn sliders(rules: &RuleSet, quantity: &Quantity) -> Vec<Slider> {
    let mut out: Vec<Slider> = Vec::new();
    for part in &quantity.parts {
        for a in &part.axes {
            let (lo, hi) = (a.points[0], a.points[a.points.len() - 1]);
            match out.iter_mut().find(|s| s.name == a.input) {
                Some(s) => {
                    s.low = s.low.min(lo);
                    s.high = s.high.max(hi);
                }
                None => out.push(Slider {
                    name: a.input.clone(),
                    low: lo,
                    high: hi,
                    default: rules.inputs.get(&a.input).and_then(|i| i.default),
                    value: f64::NAN,
                }),
            }
        }
    }
    for s in &mut out {
        if categorical(rules, &s.name) {
            // A label a part is written at: the default when one is, else
            // the first.
            let points = written(quantity, &s.name);
            s.value = s
                .default
                .filter(|d| points.contains(d))
                .or_else(|| points.first().copied())
                .unwrap_or(s.low);
            continue;
        }
        if s.high <= s.low {
            // One point: a range around it (the value holds either side).
            let pad = (s.low.abs() * 0.1).max(1.0);
            s.low -= pad;
            s.high += pad;
        }
        s.value = s.default.unwrap_or((s.low + s.high) / 2.0);
    }
    out
}

/// What a query resolves to: the quantity, its inputs' values, the input
/// moved, the quantile.
struct Resolved<'a> {
    quantity: &'a Quantity,
    sliders: Vec<Slider>,
    inputs: IndexMap<String, f64>,
    x: usize,
    p: f64,
}

fn resolve<'a>(rules: &'a RuleSet, query: &Query) -> Result<Resolved<'a>, String> {
    let quantity = match &query.quantity {
        Some(name) => rules.quantity(name).map_err(|e| e.to_string())?,
        None => rules
            .quantities
            .values()
            .next()
            .ok_or("the rules have no quantities")?,
    };
    let mut sliders = sliders(rules, quantity);
    let mut inputs = IndexMap::new();
    for s in &mut sliders {
        let v = query.inputs.get(&s.name).copied().unwrap_or(s.value);
        // Beyond the outermost points every value holds: nothing changes.
        s.value = v.clamp(s.low, s.high);
        if categorical(rules, &s.name) {
            // The nearest label a part is written at: nothing lies between.
            let points = written(quantity, &s.name);
            if let Some(p) = points
                .iter()
                .copied()
                .min_by(|a, b| (a - s.value).abs().total_cmp(&(b - s.value).abs()))
            {
                s.value = p;
            }
        }
        inputs.insert(s.name.clone(), s.value);
    }
    let x = match &query.x {
        Some(name) => sliders
            .iter()
            .position(|s| &s.name == name)
            .ok_or_else(|| format!("{} reads no input '{name}'", quantity.name))?,
        None => 0,
    };
    Ok(Resolved {
        quantity,
        sliders,
        inputs,
        x,
        p: query.p.unwrap_or(0.5),
    })
}

/// Low, high, median, mean and the value at `p`.
#[derive(Debug, Clone, Copy)]
struct Summary {
    low: f64,
    high: f64,
    median: f64,
    mean: f64,
    at_p: f64,
}

impl Summary {
    fn of_dist(d: &Dist, p: f64) -> Self {
        let (low, high) = d.support();
        Summary {
            low,
            high,
            median: d.quantile(0.5),
            mean: d.mean(),
            at_p: d.quantile(p),
        }
    }

    fn of_joined(j: &Joined, p: f64) -> Self {
        let (low, high) = j.support();
        Summary {
            low,
            high,
            median: j.quantile(0.5),
            mean: j.mean(),
            at_p: j.quantile(p),
        }
    }

    fn json(&self) -> Json {
        json!({
            "low": self.low, "high": self.high, "median": self.median,
            "mean": self.mean, "at_p": self.at_p,
        })
    }
}

/// Low, high, median and the value at `p`, along an input.
#[derive(Debug, Clone, Default)]
struct Curves {
    low: Vec<f64>,
    high: Vec<f64>,
    median: Vec<f64>,
    at_p: Vec<f64>,
}

impl Curves {
    fn push(&mut self, low: f64, high: f64, median: f64, at_p: f64) {
        self.low.push(low);
        self.high.push(high);
        self.median.push(median);
        self.at_p.push(at_p);
    }

    fn json(&self) -> Json {
        json!({"low": self.low, "high": self.high, "median": self.median, "at_p": self.at_p})
    }

    fn values(&self) -> impl Iterator<Item = f64> + '_ {
        self.low.iter().chain(&self.high).copied()
    }
}

/// A part over one of its inputs, the others held at the values chosen.
struct PartChart {
    /// The input it is drawn over: the one moved, if it reads it, else its
    /// first; a part that reads no input is drawn flat over the input moved.
    /// `None` when the quantity reads no input.
    x: Option<String>,
    xs: Vec<f64>,
    curves: Curves,
    /// The points written on that axis: `(x, low, high, median)`.
    points: Vec<[f64; 4]>,
}

struct PartView<'a> {
    part: &'a Part,
    dist: Dist,
    summary: Summary,
    deciding: bool,
    /// The values it reads, in its axes' order.
    values: Vec<f64>,
    chart: PartChart,
}

/// Everything the component shows.
struct View<'a> {
    r: Resolved<'a>,
    summary: Summary,
    deciding: String,
    parts: Vec<PartView<'a>>,
    /// The quantity's distribution: values, its CDF, each part's.
    cdf_xs: Vec<f64>,
    cdf: Vec<f64>,
    part_cdfs: Vec<Vec<f64>>,
    /// The response along the input moved.
    xs: Vec<f64>,
    response: Curves,
    part_responses: Vec<Curves>,
}

/// The points `quantity`'s parts are written at for `input`, increasing.
fn written(quantity: &Quantity, input: &str) -> Vec<f64> {
    let mut out: Vec<f64> = quantity
        .parts
        .iter()
        .flat_map(|p| &p.axes)
        .filter(|a| a.input == input)
        .flat_map(|a| a.points.iter().copied())
        .collect();
    out.sort_by(f64::total_cmp);
    out.dedup();
    out
}

/// Whether `input` is a categorical input of `rules` (read at its labels).
fn categorical(rules: &RuleSet, input: &str) -> bool {
    rules
        .inputs
        .get(input)
        .is_some_and(|i| !i.labels.is_empty())
}

/// The values along slider `s`: a grid of `samples`, or a categorical
/// input's labels' indices.
fn grid_of(rules: &RuleSet, s: &Slider, samples: usize) -> Vec<f64> {
    if categorical(rules, &s.name) {
        let (lo, hi) = (s.low.ceil() as i64, s.high.floor() as i64);
        return (lo..=hi).map(|i| i as f64).collect();
    }
    grid(s.low, s.high, samples)
}

fn grid(low: f64, high: f64, n: usize) -> Vec<f64> {
    (0..n)
        .map(|k| low + (high - low) * k as f64 / (n - 1) as f64)
        .collect()
}

fn build<'a>(rules: &'a RuleSet, query: &Query, samples: usize) -> Result<View<'a>, String> {
    let r = resolve(rules, query)?;
    let q = r.quantity;
    let p = r.p;
    let joined = q
        .joined(&r.inputs, &rules.inputs)
        .map_err(|e| e.to_string())?;
    let summary = Summary::of_joined(&joined, p);
    let deciding = joined.deciding(p).to_string();
    let x_slider = r.sliders.get(r.x);
    let mut parts = Vec::new();
    for (part, (_, dist)) in q.parts.iter().zip(&joined.parts) {
        let values = q
            .values_of(part, &r.inputs, &rules.inputs)
            .map_err(|e| e.to_string())?;
        // Over the input moved, if it reads it, else its first; a part that
        // reads no input, flat over the input moved.
        let axis = (!part.axes.is_empty()).then(|| {
            part.axes
                .iter()
                .position(|a| x_slider.is_some_and(|s| a.input == s.name))
                .unwrap_or(0)
        });
        let s = match axis {
            Some(axis) => Some(
                r.sliders
                    .iter()
                    .find(|s| s.name == part.axes[axis].input)
                    .expect("every input a part reads has a slider"),
            ),
            None => x_slider,
        };
        let xs = match (axis, s) {
            // Over a categorical input: at its labels, nothing between.
            (Some(axis), _) if part.axes[axis].categorical() => part.axes[axis].points.clone(),
            (_, Some(s)) => grid_of(rules, s, samples),
            _ => Vec::new(),
        };
        let at = |v: f64| match axis {
            Some(axis) => {
                let mut vs = values.clone();
                vs[axis] = v;
                part.dist(&vs)
            }
            None => part.dist(&values),
        };
        let mut curves = Curves::default();
        for x in &xs {
            let d = at(*x);
            let (lo, hi) = d.support();
            curves.push(lo, hi, d.quantile(0.5), d.quantile(p));
        }
        let points = axis
            .map(|axis| {
                part.axes[axis]
                    .points
                    .iter()
                    .map(|x| {
                        let d = at(*x);
                        let (lo, hi) = d.support();
                        [*x, lo, hi, d.quantile(0.5)]
                    })
                    .collect()
            })
            .unwrap_or_default();
        parts.push(PartView {
            part,
            dist: *dist,
            summary: Summary::of_dist(dist, p),
            deciding: q.parts.len() > 1 && part.name == deciding,
            values,
            chart: PartChart {
                x: s.map(|s| s.name.clone()),
                xs,
                curves,
                points,
            },
        });
    }
    // The distribution, a little beyond its support.
    let (lo, hi) = (summary.low, summary.high);
    let pad = ((hi - lo) * 0.06).max(lo.abs().max(1.0) * 1e-3);
    let cdf_xs = grid(lo - pad, hi + pad, samples);
    let cdf = cdf_xs.iter().map(|x| joined.cdf(*x)).collect();
    let part_cdfs = joined
        .parts
        .iter()
        .map(|(_, d)| cdf_xs.iter().map(|x| d.cdf(*x)).collect())
        .collect();
    // The response (none for a quantity that reads no input).
    let mut response = Curves::default();
    let mut part_responses = vec![Curves::default(); q.parts.len()];
    let mut inputs = r.inputs.clone();
    let (grid_xs, x_name) = match r.sliders.get(r.x) {
        Some(s) => (grid_of(rules, s, samples), s.name.clone()),
        None => (Vec::new(), String::new()),
    };
    let x_categorical = categorical(rules, &x_name);
    let mut xs = Vec::with_capacity(grid_xs.len());
    for x in &grid_xs {
        inputs.insert(x_name.clone(), *x);
        let j = match q.joined(&inputs, &rules.inputs) {
            Ok(j) => j,
            // A label a part is not written at: no response there.
            Err(_) if x_categorical => continue,
            Err(e) => return Err(e.to_string()),
        };
        xs.push(*x);
        let (lo, hi) = j.support();
        response.push(lo, hi, j.quantile(0.5), j.quantile(p));
        for (c, (_, d)) in part_responses.iter_mut().zip(&j.parts) {
            let (lo, hi) = d.support();
            c.push(lo, hi, d.quantile(0.5), d.quantile(p));
        }
    }
    Ok(View {
        r,
        summary,
        deciding,
        parts,
        cdf_xs,
        cdf,
        part_cdfs,
        xs,
        response,
        part_responses,
    })
}

/// The component's data, as JSON: the quantity, its inputs, what it and each
/// part give at them, its distribution there (the CDF), the response along
/// the input moved, and each part over one of its inputs.
pub fn data(rules: &RuleSet, query: &Query, samples: usize) -> Result<Json, String> {
    let v = build(rules, query, samples)?;
    let q = v.r.quantity;
    let parts: Vec<Json> = v
        .parts
        .iter()
        .map(|pv| {
            let params: serde_json::Map<String, Json> = pv
                .dist
                .family()
                .params()
                .iter()
                .zip(pv.dist.params())
                .map(|(k, x)| (k.to_string(), json!(x)))
                .collect();
            json!({
                "name": pv.part.name,
                "text": pv.part.text,
                "family": pv.dist.family().name(),
                "reads": pv.part.axes.iter().zip(&pv.values).map(|(a, x)| json!({
                    "input": a.input, "points": a.points, "value": x,
                })).collect::<Vec<_>>(),
                "params": params,
                "outputs": pv.summary.json(),
                "deciding": pv.part.name == v.deciding,
                "chart": {
                    "x": pv.chart.x,
                    "xs": pv.chart.xs,
                    "curves": pv.chart.curves.json(),
                    "points": pv.chart.points.iter().map(|[x, lo, hi, m]| json!({
                        "x": x, "low": lo, "high": hi, "median": m,
                    })).collect::<Vec<_>>(),
                },
            })
        })
        .collect();
    Ok(json!({
        "quantity": q.name,
        "source": q.source,
        "text": q.text,
        "combine": q.combine.name(),
        "quantities": rules.quantities.keys().collect::<Vec<_>>(),
        "p": v.r.p,
        "inputs": v.r.sliders.iter().map(|s| json!({
            "name": s.name, "range": [s.low, s.high], "default": s.default, "value": s.value,
        })).collect::<Vec<_>>(),
        "outputs": v.summary.json(),
        "deciding": v.deciding,
        "parts": parts,
        "distribution": {
            "xs": v.cdf_xs,
            "cdf": v.cdf,
            "parts": v.parts.iter().zip(&v.part_cdfs).map(|(pv, c)| json!({
                "name": pv.part.name, "cdf": c,
            })).collect::<Vec<_>>(),
        },
        "response": {
            "x": v.r.sliders.get(v.r.x).map(|s| &s.name),
            "xs": v.xs,
            "curves": v.response.json(),
            "parts": v.parts.iter().zip(&v.part_responses).map(|(pv, c)| json!({
                "name": pv.part.name, "curves": c.json(),
            })).collect::<Vec<_>>(),
        },
    }))
}

/// A number as the component writes it: four significant digits at most.
pub fn num(v: f64) -> String {
    if !v.is_finite() {
        return v.to_string();
    }
    let a = v.abs();
    let decimals = if a >= 1000.0 {
        0
    } else if a >= 100.0 {
        1
    } else if a >= 10.0 {
        2
    } else {
        3
    };
    let s = format!("{v:.decimals$}");
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

fn nice_ticks(lo: f64, hi: f64, n: usize) -> Vec<f64> {
    let span = hi - lo;
    if span.is_nan() || span <= 0.0 {
        return vec![lo];
    }
    let raw = span / n as f64;
    let mag = 10f64.powf(raw.log10().floor());
    let step = [1.0, 2.0, 2.5, 5.0, 10.0]
        .iter()
        .map(|m| m * mag)
        .find(|s| span / s <= n as f64)
        .unwrap_or(10.0 * mag);
    let first = (lo / step).ceil() * step;
    let mut out = Vec::new();
    let mut t = first;
    while t <= hi + step * 1e-9 {
        out.push(if t.abs() < step * 1e-9 { 0.0 } else { t });
        t += step;
    }
    out
}

/// The least and greatest of `values`, padded a little.
fn y_range(values: impl Iterator<Item = f64>) -> (f64, f64) {
    let (y0, y1) = values.fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), v| {
        (a.min(v), b.max(v))
    });
    let pad = ((y1 - y0) * 0.08).max(y0.abs().max(1.0) * 1e-3);
    (y0 - pad, y1 + pad)
}

/// A chart's frame: the plot area inside the viewBox, and its scales.
struct Frame {
    w: f64,
    h: f64,
    left: f64,
    right: f64,
    top: f64,
    bottom: f64,
    x0: f64,
    x1: f64,
    y0: f64,
    y1: f64,
}

impl Frame {
    fn new(w: f64, h: f64, x: (f64, f64), y: (f64, f64)) -> Self {
        Frame {
            w,
            h,
            left: 44.0,
            right: 12.0,
            top: 10.0,
            bottom: 24.0,
            x0: x.0,
            x1: x.1,
            y0: y.0,
            y1: y.1,
        }
    }

    fn px(&self, x: f64) -> f64 {
        let t = if self.x1 > self.x0 {
            (x - self.x0) / (self.x1 - self.x0)
        } else {
            0.5
        };
        self.left + t * (self.w - self.left - self.right)
    }

    fn py(&self, y: f64) -> f64 {
        let t = if self.y1 > self.y0 {
            (y - self.y0) / (self.y1 - self.y0)
        } else {
            0.5
        };
        self.h - self.bottom - t * (self.h - self.top - self.bottom)
    }

    fn open(&self, out: &mut String, label: &str) {
        out.push_str(&format!(
            "<svg class=\"oo-rules-chart\" viewBox=\"0 0 {} {}\" role=\"img\" aria-label=\"{}\">",
            self.w,
            self.h,
            escape(label)
        ));
        let (bx, by) = (self.h - self.bottom, self.left);
        for t in nice_ticks(self.y0, self.y1, 4) {
            let y = self.py(t);
            out.push_str(&format!(
                "<line class=\"oo-grid\" x1=\"{by:.1}\" x2=\"{:.1}\" y1=\"{y:.1}\" y2=\"{y:.1}\"/>\
                 <text class=\"oo-tick\" x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"end\">{}</text>",
                self.w - self.right,
                by - 6.0,
                y + 3.5,
                num(t)
            ));
        }
        for t in nice_ticks(self.x0, self.x1, 5) {
            let x = self.px(t);
            out.push_str(&format!(
                "<text class=\"oo-tick\" x=\"{x:.1}\" y=\"{:.1}\" text-anchor=\"middle\">{}</text>",
                bx + 15.0,
                num(t)
            ));
        }
        out.push_str(&format!(
            "<line class=\"oo-axis\" x1=\"{by:.1}\" x2=\"{:.1}\" y1=\"{bx:.1}\" y2=\"{bx:.1}\"/>",
            self.w - self.right
        ));
    }

    fn path(&self, xs: &[f64], ys: &[f64]) -> String {
        let mut d = String::new();
        for (k, (x, y)) in xs.iter().zip(ys).enumerate() {
            d.push_str(&format!(
                "{}{:.1},{:.1}",
                if k == 0 { "M" } else { "L" },
                self.px(*x),
                self.py(*y)
            ));
        }
        d
    }

    /// The area between `lo` and `hi` along `xs`.
    fn band(&self, xs: &[f64], lo: &[f64], hi: &[f64]) -> String {
        let mut d = self.path(xs, hi);
        for (x, y) in xs.iter().zip(lo).rev() {
            d.push_str(&format!("L{:.1},{:.1}", self.px(*x), self.py(*y)));
        }
        d.push('Z');
        d
    }

    /// A vertical line at `x`.
    fn marker(&self, out: &mut String, x: f64) {
        let px = self.px(x);
        out.push_str(&format!(
            "<line class=\"oo-rules-at\" x1=\"{px:.1}\" x2=\"{px:.1}\" y1=\"{:.1}\" y2=\"{:.1}\"/>",
            self.top,
            self.h - self.bottom
        ));
    }

    /// A horizontal line at `y`.
    fn level(&self, out: &mut String, y: f64) {
        let py = self.py(y);
        out.push_str(&format!(
            "<line class=\"oo-rules-at\" x1=\"{:.1}\" x2=\"{:.1}\" y1=\"{py:.1}\" y2=\"{py:.1}\"/>",
            self.left,
            self.w - self.right
        ));
    }
}

fn picker(out: &mut String, what: &str, label: &str, options: &[String], chosen: &str) {
    out.push_str(&format!(
        "<label class=\"oo-pick\"><span>{}</span><select data-oo-rules=\"{what}\">",
        escape(label)
    ));
    for o in options {
        out.push_str(&format!(
            "<option value=\"{0}\"{1}>{0}</option>",
            escape(o),
            if o == chosen { " selected" } else { "" }
        ));
    }
    out.push_str("</select></label>");
}

fn summary_cells(out: &mut String, s: &Summary, class: &str, win: bool) {
    for v in [s.low, s.high, s.median, s.mean] {
        out.push_str(&format!("<td class=\"oo-num{class}\">{}</td>", num(v)));
    }
    out.push_str(&format!(
        "<td class=\"oo-num{class}{}\">{}</td>",
        if win { " oo-win" } else { "" },
        num(s.at_p)
    ));
}

fn outputs_table(out: &mut String, v: &View, labels: &Labels) {
    out.push_str(&format!(
        "<h4>{}</h4><table class=\"oo-rules-table\"><thead><tr><th></th>",
        escape(&labels.outputs)
    ));
    for l in [&labels.low, &labels.high, &labels.median, &labels.mean] {
        out.push_str(&format!("<th class=\"oo-num\">{}</th>", escape(l)));
    }
    out.push_str(&format!(
        "<th class=\"oo-num\">{} = {}</th></tr></thead><tbody>",
        escape(&labels.at_p),
        num(v.r.p)
    ));
    out.push_str(&format!(
        "<tr class=\"oo-rules-total\"><th>{}</th>",
        escape(&v.r.quantity.name)
    ));
    summary_cells(out, &v.summary, " oo-rules-value", false);
    out.push_str("</tr>");
    if v.parts.len() > 1 {
        for (k, pv) in v.parts.iter().enumerate() {
            out.push_str(&format!(
                "<tr{}><th><i class=\"oo-rules-key oo-rules-s{}\"></i>{}{}</th>",
                if pv.deciding {
                    " class=\"oo-rules-deciding\""
                } else {
                    ""
                },
                k % 8,
                escape(&pv.part.name),
                if pv.deciding {
                    format!(
                        " <span class=\"oo-rules-tag\">{}</span>",
                        escape(&labels.deciding)
                    )
                } else {
                    String::new()
                }
            ));
            summary_cells(out, &pv.summary, "", pv.deciding);
            out.push_str("</tr>");
        }
    }
    out.push_str("</tbody></table>");
}

fn distribution_chart(out: &mut String, v: &View, labels: &Labels) {
    let q = v.r.quantity;
    out.push_str(&format!(
        "<section class=\"oo-rules-dist\"><h4>{}</h4>",
        escape(&labels.distribution)
    ));
    let f = Frame::new(
        640.0,
        180.0,
        (v.cdf_xs[0], v.cdf_xs[v.cdf_xs.len() - 1]),
        (0.0, 1.0),
    );
    f.open(out, &format!("{}: {}", q.name, labels.distribution));
    if v.parts.len() > 1 {
        for (k, (pv, c)) in v.parts.iter().zip(&v.part_cdfs).enumerate() {
            out.push_str(&format!(
                "<path class=\"oo-rules-base oo-rules-s{}\" d=\"{}\"><title>{}</title></path>",
                k % 8,
                f.path(&v.cdf_xs, c),
                escape(&pv.part.name)
            ));
        }
    }
    out.push_str(&format!(
        "<path class=\"oo-rules-out oo-rules-o0\" d=\"{}\"><title>{}</title></path>",
        f.path(&v.cdf_xs, &v.cdf),
        escape(&q.name)
    ));
    f.level(out, v.r.p);
    f.marker(out, v.summary.at_p);
    out.push_str(&format!(
        "<circle class=\"oo-rules-dot oo-rules-o1\" cx=\"{:.1}\" cy=\"{:.1}\" r=\"4\"><title>{} = {}: {}</title></circle>",
        f.px(v.summary.at_p),
        f.py(v.r.p),
        escape(&labels.at_p),
        num(v.r.p),
        num(v.summary.at_p)
    ));
    out.push_str("</svg><ul class=\"oo-rules-legend\">");
    out.push_str(&format!(
        "<li><i class=\"oo-rules-o0\"></i>{} (CDF)</li><li><i class=\"oo-rules-o1\"></i>{} = {} <b>{}</b></li>",
        escape(&q.name),
        escape(&labels.at_p),
        num(v.r.p),
        num(v.summary.at_p)
    ));
    if v.parts.len() > 1 {
        for (k, pv) in v.parts.iter().enumerate() {
            out.push_str(&format!(
                "<li class=\"oo-rules-dash\"><i class=\"oo-rules-s{}\"></i>{}</li>",
                k % 8,
                escape(&pv.part.name)
            ));
        }
    }
    out.push_str("</ul></section>");
}

fn response_chart(out: &mut String, v: &View, labels: &Labels) {
    let q = v.r.quantity;
    // A quantity that reads no input has no response to show.
    let Some(x) = v.r.sliders.get(v.r.x) else {
        return;
    };
    out.push_str("<section class=\"oo-rules-response\"><div class=\"oo-rules-row\">");
    out.push_str(&format!("<h4>{}</h4>", escape(&labels.response)));
    let names: Vec<String> = v.r.sliders.iter().map(|s| s.name.clone()).collect();
    picker(out, "x", &labels.vary, &names, &x.name);
    out.push_str("</div>");
    let several = v.parts.len() > 1;
    let (y0, y1) = y_range(
        v.response.values().chain(
            v.part_responses
                .iter()
                .filter(|_| several)
                .flat_map(Curves::values),
        ),
    );
    let f = Frame::new(640.0, 220.0, (x.low, x.high), (y0, y1));
    f.open(out, &format!("{} / {}", q.name, x.name));
    out.push_str(&format!(
        "<path class=\"oo-rules-band\" d=\"{}\"/>",
        f.band(&v.xs, &v.response.low, &v.response.high)
    ));
    if several {
        for (k, (pv, c)) in v.parts.iter().zip(&v.part_responses).enumerate() {
            for (what, ys) in [(&labels.low, &c.low), (&labels.high, &c.high)] {
                out.push_str(&format!(
                    "<path class=\"oo-rules-base oo-rules-s{}\" d=\"{}\"><title>{} · {}</title></path>",
                    k % 8,
                    f.path(&v.xs, ys),
                    escape(&pv.part.name),
                    escape(what)
                ));
            }
        }
    }
    for (k, (what, ys)) in [
        (&labels.median, &v.response.median),
        (&labels.at_p, &v.response.at_p),
    ]
    .into_iter()
    .enumerate()
    {
        out.push_str(&format!(
            "<path class=\"oo-rules-out oo-rules-o{k}\" d=\"{}\"><title>{}</title></path>",
            f.path(&v.xs, ys),
            escape(what)
        ));
    }
    f.marker(out, x.value);
    out.push_str("</svg><ul class=\"oo-rules-legend\">");
    out.push_str(&format!(
        "<li class=\"oo-rules-swatch\"><i></i>{}–{} <b>{} – {}</b></li>\
         <li><i class=\"oo-rules-o0\"></i>{} <b>{}</b></li>\
         <li><i class=\"oo-rules-o1\"></i>{} = {} <b>{}</b></li>",
        escape(&labels.low),
        escape(&labels.high),
        num(v.summary.low),
        num(v.summary.high),
        escape(&labels.median),
        num(v.summary.median),
        escape(&labels.at_p),
        num(v.r.p),
        num(v.summary.at_p)
    ));
    if several {
        for (k, pv) in v.parts.iter().enumerate() {
            out.push_str(&format!(
                "<li class=\"oo-rules-dash\"><i class=\"oo-rules-s{}\"></i>{}</li>",
                k % 8,
                escape(&pv.part.name)
            ));
        }
    }
    out.push_str("</ul></section>");
}

fn part_chart(out: &mut String, pv: &PartView, k: usize, v: &View, labels: &Labels) {
    let part = pv.part;
    let s = pv
        .chart
        .x
        .as_ref()
        .and_then(|x| v.r.sliders.iter().find(|s| &s.name == x));
    out.push_str(&format!(
        "<figure class=\"oo-rules-part{}\"><figcaption><span class=\"oo-rules-name\">{}</span> \
         <span class=\"oo-hint\">{} · {}</span>",
        if pv.deciding {
            " oo-rules-deciding"
        } else {
            ""
        },
        escape(&part.name),
        pv.dist.family().name(),
        part.axes
            .iter()
            .zip(&pv.values)
            .map(|(a, x)| format!("{} = {}", escape(&a.input), num(*x)))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    if pv.deciding {
        out.push_str(&format!(
            " <span class=\"oo-rules-tag\">{}</span>",
            escape(&labels.deciding)
        ));
    }
    if !part.text.is_empty() {
        out.push_str(&format!("<p class=\"oo-hint\">{}</p>", escape(&part.text)));
    }
    out.push_str("</figcaption>");
    // Over an input; a part of a quantity that reads none has its legend only.
    if let Some(s) = s {
        part_svg(out, pv, k, v, labels, s);
    }
    out.push_str("<ul class=\"oo-rules-legend\">");
    let params: Vec<String> = pv
        .dist
        .family()
        .params()
        .iter()
        .zip(pv.dist.params())
        .map(|(n, x)| format!("{n} <b>{}</b>", num(x)))
        .collect();
    out.push_str(&format!(
        "<li class=\"oo-rules-swatch\"><i class=\"oo-rules-part-key oo-rules-s{s}\"></i>{}</li>",
        params.join(" "),
        s = k % 8
    ));
    if !pv.chart.points.is_empty() {
        out.push_str(&format!(
            "<li class=\"oo-rules-dots\"><i class=\"oo-rules-s{}\"></i>{}</li>",
            k % 8,
            escape(&labels.points)
        ));
    }
    out.push_str("</ul></figure>");
}

/// A part's chart over the input `s`: its band, median and the value at `p`,
/// the points written, and a marker at the value chosen.
fn part_svg(out: &mut String, pv: &PartView, k: usize, v: &View, labels: &Labels, s: &Slider) {
    let part = pv.part;
    let c = &pv.chart.curves;
    let (y0, y1) = y_range(c.values());
    let f = Frame::new(320.0, 170.0, (s.low, s.high), (y0, y1));
    f.open(out, &format!("{} / {}", part.name, s.name));
    let xs = &pv.chart.xs;
    out.push_str(&format!(
        "<path class=\"oo-rules-area oo-rules-s{s}\" d=\"{}\"/>\
         <path class=\"oo-rules-line oo-rules-s{s}\" d=\"{}\"><title>{}</title></path>\
         <path class=\"oo-rules-pline\" d=\"{}\"><title>{} = {}</title></path>",
        f.band(xs, &c.low, &c.high),
        f.path(xs, &c.median),
        escape(&labels.median),
        f.path(xs, &c.at_p),
        escape(&labels.at_p),
        num(v.r.p),
        s = k % 8
    ));
    for [x, lo, hi, m] in &pv.chart.points {
        let title = format!(
            "{} = {}: {} – {}, {} {}",
            s.name,
            num(*x),
            num(*lo),
            num(*hi),
            labels.median,
            num(*m)
        );
        for y in [lo, hi, m] {
            out.push_str(&format!(
                "<circle class=\"oo-rules-pt oo-rules-s{}\" cx=\"{:.1}\" cy=\"{:.1}\" r=\"3\"><title>{}</title></circle>",
                k % 8,
                f.px(*x),
                f.py(*y),
                escape(&title)
            ));
        }
    }
    f.marker(out, s.value);
    out.push_str("</svg>");
}

/// The component, rendered at `query`.
pub fn html(rules: &RuleSet, query: &Query, opts: &Options) -> Result<String, String> {
    let v = build(rules, query, opts.samples)?;
    let q = v.r.quantity;
    let labels = &opts.labels;
    let mut out = String::new();
    out.push_str(&format!(
        "<div class=\"oo-cov oo-rules\"{} data-oo-quantity=\"{}\">",
        if opts.id.is_empty() {
            String::new()
        } else {
            format!(" id=\"{}\"", escape(&opts.id))
        },
        escape(&q.name)
    ));
    // Head.
    out.push_str(&format!(
        "<div class=\"oo-head\"><h3>{}</h3><p class=\"oo-hint\">{} · {} {} · {} {}</p>",
        escape(&q.name),
        escape(&q.source),
        q.parts.len(),
        escape(if q.parts.len() == 1 {
            &labels.part
        } else {
            &labels.parts
        }),
        escape(&labels.combine),
        q.combine.name()
    ));
    if !q.text.is_empty() {
        out.push_str(&format!("<p class=\"oo-hint\">{}</p>", escape(&q.text)));
    }
    out.push_str("</div>");
    if rules.quantities.len() > 1 {
        out.push_str("<div class=\"oo-rules-bar-top\">");
        let names: Vec<String> = rules.quantities.keys().cloned().collect();
        picker(&mut out, "quantity", &labels.quantity, &names, &q.name);
        out.push_str("</div>");
    }
    // Inputs and outputs.
    out.push_str("<div class=\"oo-rules-io\"><section class=\"oo-rules-inputs\">");
    out.push_str(&format!("<h4>{}</h4>", escape(&labels.inputs)));
    for s in &v.r.sliders {
        let step = (s.high - s.low) / 200.0;
        out.push_str(&format!(
            "<label class=\"oo-rules-slider\"><span class=\"oo-rules-name\">{0}</span>\
             <input type=\"range\" data-oo-input=\"{0}\" min=\"{1}\" max=\"{2}\" step=\"{3}\" value=\"{4}\">\
             <output>{5}</output></label>",
            escape(&s.name),
            s.low,
            s.high,
            step,
            s.value,
            num(s.value)
        ));
    }
    out.push_str(&format!(
        "<label class=\"oo-rules-slider oo-rules-p\"><span>{}</span>\
         <input type=\"range\" data-oo-p min=\"0\" max=\"1\" step=\"0.01\" value=\"{}\">\
         <output>{}</output></label>",
        escape(&labels.quantile),
        v.r.p,
        num(v.r.p)
    ));
    out.push_str("</section><section class=\"oo-rules-result\">");
    outputs_table(&mut out, &v, labels);
    out.push_str("</section></div>");
    distribution_chart(&mut out, &v, labels);
    response_chart(&mut out, &v, labels);
    // Parts.
    out.push_str("<section class=\"oo-rules-parts\"><div class=\"oo-rules-grid\">");
    for (k, pv) in v.parts.iter().enumerate() {
        part_chart(&mut out, pv, k, &v, labels);
    }
    out.push_str("</div></section></div>");
    Ok(out)
}

/// The component's CSS, on the coverage view's (which hosts put on the page
/// with it).
pub fn styles() -> &'static str {
    include_str!("../assets/rules_view.css")
}

/// The component's script: `openodd:rules` from its sliders and pickers.
pub fn script() -> &'static str {
    include_str!("../assets/rules_view.js")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_are_short() {
        assert_eq!(num(0.5700000000000001), "0.57");
        assert_eq!(num(250.0), "250");
        assert_eq!(num(-0.0001), "0");
        assert_eq!(num(1234.56), "1235");
        assert_eq!(num(12.345), "12.35");
    }

    #[test]
    fn ticks_are_round() {
        assert_eq!(
            nice_ticks(0.0, 100.0, 5),
            vec![0.0, 20.0, 40.0, 60.0, 80.0, 100.0]
        );
        assert_eq!(nice_ticks(-90.0, 90.0, 5), vec![-50.0, 0.0, 50.0]);
    }
}
