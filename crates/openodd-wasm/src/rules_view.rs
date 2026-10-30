//! The fuzzy rules of a backend, as a component: what each input's terms
//! are, which rules fire at the inputs chosen, what each rule base and the
//! quantity give there, and how the quantity answers one input moved across
//! its universe.
//!
//! [`html`] renders a fragment (`<div class="oo-cov oo-rules">`) on the
//! coverage view's styles and theme variables, plus [`styles`] and
//! [`script`] of its own; the script dispatches `openodd:rules` from its
//! sliders and pickers, which `js/viewer.js` (`mount`, `view: "rules"`)
//! answers by rendering again. [`data`] is the same as JSON, for a host that
//! draws it its own way.

use indexmap::IndexMap;
use openodd::rules::{Input, Membership, Quantity, RuleBase, RuleSet};
use openodd_coverage_view::html::escape;
use serde_json::{json, Value as Json};

/// Which quantity to show, at which inputs, moving which one.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Query {
    /// The quantity (the first, if `None`).
    pub quantity: Option<String>,
    /// Input -> its value; one not given reads its default, else the middle
    /// of its universe.
    pub inputs: IndexMap<String, f64>,
    /// The input the response chart moves (the quantity's first, if `None`).
    pub x: Option<String>,
}

impl Query {
    /// From JSON: `{quantity, inputs: {name: value}, x}`.
    pub fn from_json(raw: &Json) -> Result<Self, String> {
        let mut q = Query {
            quantity: raw.get("quantity").and_then(Json::as_str).map(String::from),
            x: raw.get("x").and_then(Json::as_str).map(String::from),
            ..Default::default()
        };
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
    pub combined: String,
    pub response: String,
    pub vary: String,
    pub rules: String,
    pub firing: String,
    pub terms: String,
    pub quantity: String,
    pub combine: String,
    pub rule_base: String,
    pub rule_bases: String,
}

impl Default for Labels {
    fn default() -> Self {
        Labels {
            inputs: "Inputs".into(),
            outputs: "Outputs".into(),
            combined: "Combined".into(),
            response: "Response".into(),
            vary: "Vary".into(),
            rules: "Rules".into(),
            firing: "Firing".into(),
            terms: "Terms".into(),
            quantity: "Quantity".into(),
            combine: "combine".into(),
            rule_base: "rule base".into(),
            rule_bases: "rule bases".into(),
        }
    }
}

impl Labels {
    /// Set one label by its field name; `false` if there is no such label.
    pub fn set(&mut self, name: &str, value: &str) -> bool {
        let slot = match name {
            "inputs" => &mut self.inputs,
            "outputs" => &mut self.outputs,
            "combined" => &mut self.combined,
            "response" => &mut self.response,
            "vary" => &mut self.vary,
            "rules" => &mut self.rules,
            "firing" => &mut self.firing,
            "terms" => &mut self.terms,
            "quantity" => &mut self.quantity,
            "combine" => &mut self.combine,
            "rule_base" => &mut self.rule_base,
            "rule_bases" => &mut self.rule_bases,
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

/// An input of a quantity, over every rule base reading it.
#[derive(Debug, Clone, PartialEq)]
pub struct Slider {
    pub name: String,
    pub low: f64,
    pub high: f64,
    pub value: f64,
}

/// What a query resolves to: the quantity, its inputs' values, the input moved.
struct Resolved<'a> {
    quantity: &'a Quantity,
    sliders: Vec<Slider>,
    inputs: IndexMap<String, f64>,
    x: usize,
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
    let mut sliders: Vec<Slider> = Vec::new();
    let mut defaults: IndexMap<String, f64> = IndexMap::new();
    for base in &quantity.rule_bases {
        for i in &base.inputs {
            if let Some(d) = i.default {
                defaults.entry(i.name.clone()).or_insert(d);
            }
            match sliders.iter_mut().find(|s| s.name == i.name) {
                Some(s) => {
                    s.low = s.low.min(i.low);
                    s.high = s.high.max(i.high);
                }
                None => sliders.push(Slider {
                    name: i.name.clone(),
                    low: i.low,
                    high: i.high,
                    value: f64::NAN,
                }),
            }
        }
    }
    let mut inputs = IndexMap::new();
    for s in &mut sliders {
        let v = query
            .inputs
            .get(&s.name)
            .or(defaults.get(&s.name))
            .copied()
            .unwrap_or((s.low + s.high) / 2.0);
        s.value = v.clamp(s.low, s.high);
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
    })
}

/// The quantity's outputs, and each rule base's, along the input moved.
struct Response {
    xs: Vec<f64>,
    combined: IndexMap<String, Vec<f64>>,
    bases: Vec<(String, IndexMap<String, Vec<f64>>)>,
}

fn response(r: &Resolved, samples: usize) -> Result<Response, String> {
    let s = &r.sliders[r.x];
    let xs: Vec<f64> = (0..samples)
        .map(|k| s.low + (s.high - s.low) * k as f64 / (samples - 1) as f64)
        .collect();
    let outputs = &r.quantity.outputs;
    let mut combined: IndexMap<String, Vec<f64>> =
        outputs.iter().map(|o| (o.clone(), Vec::new())).collect();
    let mut bases: Vec<(String, IndexMap<String, Vec<f64>>)> = r
        .quantity
        .rule_bases
        .iter()
        .map(|b| {
            (
                b.name.clone(),
                outputs.iter().map(|o| (o.clone(), Vec::new())).collect(),
            )
        })
        .collect();
    let mut inputs = r.inputs.clone();
    for x in &xs {
        inputs.insert(s.name.clone(), *x);
        let each = r
            .quantity
            .evaluate_each(&inputs)
            .map_err(|e| e.to_string())?;
        let joined = r.quantity.evaluate(&inputs).map_err(|e| e.to_string())?;
        for o in outputs {
            combined[o].push(joined[o]);
        }
        for (k, (_, out)) in each.iter().enumerate() {
            for o in outputs {
                bases[k].1[o].push(out[o]);
            }
        }
    }
    Ok(Response {
        xs,
        combined,
        bases,
    })
}

/// The component's data, as JSON: the quantity, its inputs, each rule base's
/// terms (sampled), rules (with how strongly each fires) and outputs, and the
/// response along the input moved.
pub fn data(rules: &RuleSet, query: &Query, samples: usize) -> Result<Json, String> {
    let r = resolve(rules, query)?;
    let each = r
        .quantity
        .evaluate_each(&r.inputs)
        .map_err(|e| e.to_string())?;
    let combined = r.quantity.evaluate(&r.inputs).map_err(|e| e.to_string())?;
    let resp = response(&r, samples)?;
    let mut bases = Vec::new();
    for (base, (_, outputs)) in r.quantity.rule_bases.iter().zip(&each) {
        let values = r
            .quantity
            .values_of(base, &r.inputs)
            .map_err(|e| e.to_string())?;
        let firing = base.firing(&values);
        let inputs: Vec<Json> = base
            .inputs
            .iter()
            .zip(&values)
            .map(|(i, v)| {
                let xs = grid(i.low, i.high, samples);
                json!({
                    "name": i.name, "universe": [i.low, i.high], "default": i.default,
                    "value": i.clamp(*v),
                    "terms": i.terms.iter().map(|(t, m)| json!({
                        "name": t, "shape": shape(m), "degree": m.degree(i.clamp(*v)),
                        "curve": xs.iter().map(|x| [*x, m.degree(*x)]).collect::<Vec<_>>(),
                    })).collect::<Vec<_>>(),
                })
            })
            .collect();
        let rules: Vec<Json> = base
            .rules
            .iter()
            .zip(&firing)
            .map(|(rule, f)| json!({"if": rule.antecedent, "then": rule.then, "firing": f}))
            .collect();
        bases.push(json!({
            "name": base.name, "inputs": inputs, "rules": rules, "outputs": outputs,
        }));
    }
    Ok(json!({
        "quantity": r.quantity.name,
        "source": r.quantity.source,
        "combine": combine_name(r.quantity),
        "quantities": rules.quantities.keys().collect::<Vec<_>>(),
        "outputs": r.quantity.outputs,
        "inputs": r.sliders.iter().map(|s| json!({
            "name": s.name, "range": [s.low, s.high], "value": s.value,
        })).collect::<Vec<_>>(),
        "combined": combined,
        "rule_bases": bases,
        "response": {
            "x": r.sliders[r.x].name,
            "xs": resp.xs,
            "combined": resp.combined,
            "rule_bases": resp.bases.iter().map(|(n, o)| json!({"name": n, "outputs": o})).collect::<Vec<_>>(),
        },
    }))
}

fn combine_name(q: &Quantity) -> &'static str {
    match q.combine {
        openodd::rules::Combine::Min => "min",
        openodd::rules::Combine::Max => "max",
    }
}

fn shape(m: &Membership) -> Json {
    match m {
        Membership::Triangle(p) => json!({"triangle": p}),
        Membership::Trapezoid(p) => json!({"trapezoid": p}),
        Membership::Gaussian { mean, sigma } => json!({"gaussian": [mean, sigma]}),
    }
}

fn grid(low: f64, high: f64, n: usize) -> Vec<f64> {
    (0..n)
        .map(|k| low + (high - low) * k as f64 / (n - 1) as f64)
        .collect()
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

    fn marker(&self, out: &mut String, x: f64) {
        let px = self.px(x);
        out.push_str(&format!(
            "<line class=\"oo-rules-at\" x1=\"{px:.1}\" x2=\"{px:.1}\" y1=\"{:.1}\" y2=\"{:.1}\"/>",
            self.top,
            self.h - self.bottom
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

fn membership_chart(out: &mut String, input: &Input, value: f64, samples: usize, labels: &Labels) {
    let f = Frame::new(320.0, 150.0, (input.low, input.high), (0.0, 1.0));
    out.push_str("<figure class=\"oo-rules-mf\">");
    out.push_str(&format!(
        "<figcaption><span class=\"oo-rules-name\">{}</span> <span class=\"oo-hint\">[{}, {}]</span></figcaption>",
        escape(&input.name),
        num(input.low),
        num(input.high)
    ));
    f.open(out, &format!("{}: {}", input.name, labels.terms));
    let xs = grid(input.low, input.high, samples);
    for (k, m) in input.terms.values().enumerate() {
        let ys: Vec<f64> = xs.iter().map(|x| m.degree(*x)).collect();
        let line = f.path(&xs, &ys);
        let base = f.py(0.0);
        out.push_str(&format!(
            "<path class=\"oo-rules-area oo-rules-s{s}\" d=\"{line}L{:.1},{base:.1}L{:.1},{base:.1}Z\"/>\
             <path class=\"oo-rules-line oo-rules-s{s}\" d=\"{line}\"/>",
            f.px(input.high),
            f.px(input.low),
            s = k % 8
        ));
    }
    f.marker(out, value);
    out.push_str("</svg><ul class=\"oo-rules-legend\">");
    for (k, (t, m)) in input.terms.iter().enumerate() {
        let d = m.degree(value);
        out.push_str(&format!(
            "<li class=\"{}\"><i class=\"oo-rules-s{}\"></i>{} <b>{}</b></li>",
            if d > 0.0 { "oo-on" } else { "oo-off" },
            k % 8,
            escape(t),
            num(d)
        ));
    }
    out.push_str("</ul></figure>");
}

fn rule_table(out: &mut String, base: &RuleBase, firing: &[f64], labels: &Labels) {
    out.push_str(&format!(
        "<table class=\"oo-rules-table\"><thead><tr><th>#</th><th>{}</th><th>{}</th>",
        escape(&labels.rules),
        escape(&labels.firing)
    ));
    for o in &base.outputs {
        out.push_str(&format!("<th class=\"oo-num\">{}</th>", escape(o)));
    }
    out.push_str("</tr></thead><tbody>");
    let total: f64 = firing.iter().sum();
    for (k, (rule, f)) in base.rules.iter().zip(firing).enumerate() {
        let share = if total > 0.0 { f / total } else { 0.0 };
        out.push_str(&format!(
            "<tr class=\"{}\"><td class=\"oo-num\">{}</td><td><code>{}</code></td>\
             <td><span class=\"oo-rules-bar\" title=\"{}\"><i style=\"width:{:.1}%\"></i></span>\
             <span class=\"oo-rules-w\">{}</span></td>",
            if *f > 0.0 { "oo-on" } else { "oo-off" },
            k + 1,
            escape(&rule.antecedent),
            num(share),
            f * 100.0,
            num(*f)
        ));
        for o in &base.outputs {
            out.push_str(&format!("<td class=\"oo-num\">{}</td>", num(rule.then[o])));
        }
        out.push_str("</tr>");
    }
    out.push_str("</tbody></table>");
}

/// The component, rendered at `query`.
pub fn html(rules: &RuleSet, query: &Query, opts: &Options) -> Result<String, String> {
    let r = resolve(rules, query)?;
    let q = r.quantity;
    let labels = &opts.labels;
    let each = q.evaluate_each(&r.inputs).map_err(|e| e.to_string())?;
    let combined = q.evaluate(&r.inputs).map_err(|e| e.to_string())?;
    let resp = response(&r, opts.samples)?;
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
        "<div class=\"oo-head\"><h3>{}</h3><p class=\"oo-hint\">{} · {} {} · {} {}</p></div>",
        escape(&q.name),
        escape(&q.source),
        q.rule_bases.len(),
        escape(if q.rule_bases.len() == 1 {
            &labels.rule_base
        } else {
            &labels.rule_bases
        }),
        escape(&labels.combine),
        combine_name(q)
    ));
    if rules.quantities.len() > 1 {
        out.push_str("<div class=\"oo-rules-bar-top\">");
        let names: Vec<String> = rules.quantities.keys().cloned().collect();
        picker(&mut out, "quantity", &labels.quantity, &names, &q.name);
        out.push_str("</div>");
    }
    // Inputs and outputs.
    out.push_str("<div class=\"oo-rules-io\"><section class=\"oo-rules-inputs\">");
    out.push_str(&format!("<h4>{}</h4>", escape(&labels.inputs)));
    for s in &r.sliders {
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
    out.push_str("</section><section class=\"oo-rules-result\">");
    out.push_str(&format!(
        "<h4>{}</h4><table class=\"oo-rules-table\"><thead><tr><th></th><th class=\"oo-num\">{}</th>",
        escape(&labels.outputs),
        escape(&labels.combined)
    ));
    if each.len() > 1 {
        for (name, _) in &each {
            out.push_str(&format!("<th class=\"oo-num\">{}</th>", escape(name)));
        }
    }
    out.push_str("</tr></thead><tbody>");
    for o in &q.outputs {
        out.push_str(&format!(
            "<tr><th>{}</th><td class=\"oo-num oo-rules-value\">{}</td>",
            escape(o),
            num(combined[o])
        ));
        if each.len() > 1 {
            for (_, outs) in &each {
                let wins = (outs[o] - combined[o]).abs() <= 1e-12 * combined[o].abs().max(1.0);
                out.push_str(&format!(
                    "<td class=\"oo-num{}\">{}</td>",
                    if wins { " oo-win" } else { "" },
                    num(outs[o])
                ));
            }
        }
        out.push_str("</tr>");
    }
    out.push_str("</tbody></table></section></div>");
    // Response.
    let x = &r.sliders[r.x];
    let all: Vec<f64> = resp
        .combined
        .values()
        .chain(resp.bases.iter().flat_map(|(_, o)| o.values()))
        .flatten()
        .copied()
        .collect();
    let (mut y0, mut y1) = all
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), v| {
            (a.min(*v), b.max(*v))
        });
    let pad = ((y1 - y0) * 0.08).max(1e-9);
    y0 -= pad;
    y1 += pad;
    out.push_str("<section class=\"oo-rules-response\"><div class=\"oo-rules-row\">");
    out.push_str(&format!("<h4>{}</h4>", escape(&labels.response)));
    let names: Vec<String> = r.sliders.iter().map(|s| s.name.clone()).collect();
    picker(&mut out, "x", &labels.vary, &names, &x.name);
    out.push_str("</div>");
    let f = Frame::new(640.0, 220.0, (x.low, x.high), (y0, y1));
    f.open(&mut out, &format!("{} / {}", q.name, x.name));
    let band = q.outputs.iter().any(|o| o == "low") && q.outputs.iter().any(|o| o == "high");
    if band {
        let lo = &resp.combined["low"];
        let hi = &resp.combined["high"];
        let mut d = f.path(&resp.xs, hi);
        for (xv, yv) in resp.xs.iter().zip(lo).rev() {
            d.push_str(&format!("L{:.1},{:.1}", f.px(*xv), f.py(*yv)));
        }
        d.push('Z');
        out.push_str(&format!("<path class=\"oo-rules-band\" d=\"{d}\"/>"));
    }
    if resp.bases.len() > 1 {
        for (k, (name, outs)) in resp.bases.iter().enumerate() {
            for o in &q.outputs {
                out.push_str(&format!(
                    "<path class=\"oo-rules-base oo-rules-s{}\" d=\"{}\"><title>{} · {}</title></path>",
                    (k + 1) % 8,
                    f.path(&resp.xs, &outs[o]),
                    escape(name),
                    escape(o)
                ));
            }
        }
    }
    for (k, o) in q.outputs.iter().enumerate() {
        out.push_str(&format!(
            "<path class=\"oo-rules-out oo-rules-o{}\" d=\"{}\"><title>{}</title></path>",
            k % 4,
            f.path(&resp.xs, &resp.combined[o]),
            escape(o)
        ));
    }
    f.marker(&mut out, x.value);
    out.push_str("</svg><ul class=\"oo-rules-legend\">");
    for (k, o) in q.outputs.iter().enumerate() {
        out.push_str(&format!(
            "<li><i class=\"oo-rules-o{}\"></i>{} <b>{}</b></li>",
            k % 4,
            escape(o),
            num(combined[o])
        ));
    }
    if resp.bases.len() > 1 {
        for (k, (name, _)) in resp.bases.iter().enumerate() {
            out.push_str(&format!(
                "<li class=\"oo-rules-dash\"><i class=\"oo-rules-s{}\"></i>{}</li>",
                (k + 1) % 8,
                escape(name)
            ));
        }
    }
    out.push_str("</ul></section>");
    // Rule bases.
    for base in &q.rule_bases {
        let values = q.values_of(base, &r.inputs).map_err(|e| e.to_string())?;
        let firing = base.firing(&values);
        out.push_str(&format!(
            "<section class=\"oo-rules-rb\"><h4>{}</h4><div class=\"oo-rules-mfs\">",
            escape(&base.name)
        ));
        for (input, v) in base.inputs.iter().zip(&values) {
            membership_chart(&mut out, input, input.clamp(*v), opts.samples, labels);
        }
        out.push_str("</div>");
        rule_table(&mut out, base, &firing, labels);
        out.push_str("</section>");
    }
    out.push_str("</div>");
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
