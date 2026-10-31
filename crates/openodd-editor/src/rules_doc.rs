//! A rule file ([docs/rules.md](../../../docs/rules.md)) as the rules editor
//! holds it, read from YAML and written back, and checked by openodd's own
//! reader.
//!
//! The form edits the fields a rule file has: quantities, their rule bases,
//! each base's inputs and terms, and its rules. A rule's antecedent is text;
//! one that is a conjunction of `(input IS term)` clauses, each input at most
//! once, is also offered as a row of term pickers ([`clauses`]). A file that
//! `extends` others may leave a rule base's inputs or rules to them: the
//! form keeps what the file does not name absent ([`BaseDoc::inputs`],
//! [`BaseDoc::rules`]). Comments are not kept.

use indexmap::IndexMap;
use openodd::rules::{Fetch, Origin, RuleSet};
use serde_norway::{Mapping, Value};

use crate::doc::fresh_id;

/// A term's membership function.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Shape {
    Triangle([f64; 3]),
    Trapezoid([f64; 4]),
    Gaussian { mean: f64, sigma: f64 },
}

pub const SHAPES: [&str; 3] = ["triangle", "trapezoid", "gaussian"];

impl Shape {
    pub fn kind(&self) -> &'static str {
        match self {
            Shape::Triangle(_) => "triangle",
            Shape::Trapezoid(_) => "trapezoid",
            Shape::Gaussian { .. } => "gaussian",
        }
    }

    /// The numbers the file writes, in order.
    pub fn params(&self) -> Vec<f64> {
        match *self {
            Shape::Triangle(p) => p.to_vec(),
            Shape::Trapezoid(p) => p.to_vec(),
            Shape::Gaussian { mean, sigma } => vec![mean, sigma],
        }
    }

    /// The same numbers as [`Shape::params`], with the `i`th set to `v`.
    pub fn with_param(&self, i: usize, v: f64) -> Shape {
        let mut s = *self;
        match &mut s {
            Shape::Triangle(p) => p[i.min(2)] = v,
            Shape::Trapezoid(p) => p[i.min(3)] = v,
            Shape::Gaussian { mean, sigma } => {
                if i == 0 {
                    *mean = v
                } else {
                    *sigma = v
                }
            }
        }
        s
    }

    /// The points drawn on the chart, as (value, degree): a corner to drag.
    pub fn handles(&self) -> Vec<(f64, f64)> {
        match *self {
            Shape::Triangle([a, b, c]) => vec![(a, 0.0), (b, 1.0), (c, 0.0)],
            Shape::Trapezoid([a, b, c, d]) => vec![(a, 0.0), (b, 1.0), (c, 1.0), (d, 0.0)],
            Shape::Gaussian { mean, sigma } => {
                vec![(mean, 1.0), (mean + sigma, (-0.5f64).exp())]
            }
        }
    }

    /// The shape with handle `i` dragged to `v`: kept in order between its
    /// neighbours, and within `[low, high]`.
    pub fn drag(&self, i: usize, v: f64, low: f64, high: f64) -> Shape {
        let v = v.clamp(low, high);
        match *self {
            Shape::Triangle(mut p) => {
                let lo = if i == 0 { low } else { p[i - 1] };
                let hi = if i == 2 { high } else { p[i + 1] };
                p[i] = v.clamp(lo, hi);
                Shape::Triangle(p)
            }
            Shape::Trapezoid(mut p) => {
                let lo = if i == 0 { low } else { p[i - 1] };
                let hi = if i == 3 { high } else { p[i + 1] };
                p[i] = v.clamp(lo, hi);
                Shape::Trapezoid(p)
            }
            Shape::Gaussian { mean, sigma } => {
                if i == 0 {
                    Shape::Gaussian { mean: v, sigma }
                } else {
                    let floor = (high - low).abs() / 1000.0;
                    Shape::Gaussian {
                        mean,
                        sigma: (v - mean).abs().max(floor),
                    }
                }
            }
        }
    }

    /// This shape as another kind, over about the same span.
    pub fn as_kind(&self, kind: &str) -> Shape {
        let (lo, peak, hi) = match *self {
            Shape::Triangle([a, b, c]) => (a, b, c),
            Shape::Trapezoid([a, b, c, d]) => (a, (b + c) / 2.0, d),
            Shape::Gaussian { mean, sigma } => (mean - 2.0 * sigma, mean, mean + 2.0 * sigma),
        };
        match kind {
            "triangle" => Shape::Triangle([lo, peak, hi]),
            "trapezoid" => Shape::Trapezoid([lo, (lo + peak) / 2.0, (peak + hi) / 2.0, hi]),
            _ => Shape::Gaussian {
                mean: peak,
                sigma: ((hi - lo) / 4.0).abs().max(f64::EPSILON),
            },
        }
    }

    pub fn membership(&self) -> openodd::rules::Membership {
        use openodd::rules::Membership;
        match *self {
            Shape::Triangle(p) => Membership::Triangle(p),
            Shape::Trapezoid(p) => Membership::Trapezoid(p),
            Shape::Gaussian { mean, sigma } => Membership::Gaussian { mean, sigma },
        }
    }

    pub fn degree(&self, x: f64) -> f64 {
        self.membership().degree(x)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Term {
    pub id: u64,
    pub name: String,
    pub shape: Shape,
}

#[derive(Clone, Debug, PartialEq)]
pub struct InputDoc {
    pub id: u64,
    pub name: String,
    pub low: f64,
    pub high: f64,
    pub default: Option<f64>,
    pub terms: Vec<Term>,
}

impl InputDoc {
    pub fn new(name: String) -> Self {
        let mut i = InputDoc {
            id: fresh_id(),
            name,
            low: 0.0,
            high: 100.0,
            default: None,
            terms: ["low", "medium", "high"]
                .iter()
                .map(|n| Term {
                    id: fresh_id(),
                    name: n.to_string(),
                    shape: Shape::Triangle([0.0, 0.0, 0.0]),
                })
                .collect(),
        };
        i.spread();
        i
    }

    /// Lay the terms out over the universe, in order: shoulders at the ends,
    /// triangles between, each peaking where its neighbours end. Rules naming
    /// one term each then read linearly between the peaks.
    pub fn spread(&mut self) {
        let (lo, hi, n) = (self.low, self.high, self.terms.len());
        if n == 0 {
            return;
        }
        if n == 1 {
            self.terms[0].shape = Shape::Trapezoid([lo, lo, hi, hi]);
            return;
        }
        let peak = |i: usize| tidy(lo + (hi - lo) * i as f64 / (n - 1) as f64);
        for (i, t) in self.terms.iter_mut().enumerate() {
            t.shape = if i == 0 {
                Shape::Trapezoid([lo, lo, lo, peak(1)])
            } else if i + 1 == n {
                Shape::Trapezoid([peak(n - 2), hi, hi, hi])
            } else {
                Shape::Triangle([peak(i - 1), peak(i), peak(i + 1)])
            };
        }
    }
}

/// A number without the float noise of arithmetic (`0.30000000000000004`).
pub fn tidy(v: f64) -> f64 {
    if !v.is_finite() || v == 0.0 {
        return v;
    }
    let digits = 10 - v.abs().log10().floor() as i32;
    let p = 10f64.powi(digits.clamp(0, 15));
    (v * p).round() / p
}

#[derive(Clone, Debug, PartialEq)]
pub struct RuleDoc {
    pub id: u64,
    /// The antecedent, as the file writes it.
    pub when: String,
    /// One value per output of its rule base ([`BaseDoc::outputs`]); `None`
    /// where the rule gives none.
    pub then: Vec<Option<f64>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BaseDoc {
    pub id: u64,
    pub name: String,
    /// `None`: the file does not name them (an extended file's are read).
    pub inputs: Option<Vec<InputDoc>>,
    /// The outputs its rules give, in order.
    pub outputs: Vec<String>,
    /// `None`: the file does not name them.
    pub rules: Option<Vec<RuleDoc>>,
}

impl BaseDoc {
    pub fn new(name: String, input: String, outputs: &[String]) -> Self {
        let i = InputDoc::new(input);
        let outputs: Vec<String> = if outputs.is_empty() {
            vec!["low".into(), "high".into()]
        } else {
            outputs.to_vec()
        };
        let rules = i
            .terms
            .iter()
            .map(|t| RuleDoc {
                id: fresh_id(),
                when: when_of(&[(i.name.clone(), t.name.clone())]),
                then: outputs.iter().map(|_| Some(0.0)).collect(),
            })
            .collect();
        BaseDoc {
            id: fresh_id(),
            name,
            inputs: Some(vec![i]),
            outputs,
            rules: Some(rules),
        }
    }

    /// Rename input `old` to `new`, in the inputs and in every rule.
    pub fn rename_input(&mut self, old: &str, new: &str) {
        if let Some(inputs) = &mut self.inputs {
            for i in inputs.iter_mut().filter(|i| i.name == old) {
                i.name = new.to_string();
            }
        }
        let re = regex::Regex::new(&format!(r"\(\s*{}\s+IS\s+", regex::escape(old))).unwrap();
        for r in self.rules.iter_mut().flatten() {
            r.when = re
                .replace_all(&r.when, format!("({new} IS ").as_str())
                .into_owned();
        }
    }

    /// Rename term `old` of `input` to `new`, in the input and in every rule.
    pub fn rename_term(&mut self, input: &str, old: &str, new: &str) {
        if let Some(inputs) = &mut self.inputs {
            for i in inputs.iter_mut().filter(|i| i.name == input) {
                for t in i.terms.iter_mut().filter(|t| t.name == old) {
                    t.name = new.to_string();
                }
            }
        }
        let re = regex::Regex::new(&format!(
            r"\(\s*{}\s+IS\s+{}\s*\)",
            regex::escape(input),
            regex::escape(old)
        ))
        .unwrap();
        for r in self.rules.iter_mut().flatten() {
            r.when = re
                .replace_all(&r.when, format!("({input} IS {new})").as_str())
                .into_owned();
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct QuantityDoc {
    pub id: u64,
    pub name: String,
    /// `None`: not written (`min`, unless an extended file says otherwise).
    pub combine: Option<String>,
    pub bases: Vec<BaseDoc>,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct RulesDoc {
    /// What the file `extends`, as written.
    pub extends: Option<Value>,
    pub quantities: Vec<QuantityDoc>,
}

impl RulesDoc {
    pub fn quantity(&self, id: u64) -> Option<&QuantityDoc> {
        self.quantities.iter().find(|q| q.id == id)
    }

    pub fn quantity_mut(&mut self, id: u64) -> Option<&mut QuantityDoc> {
        self.quantities.iter_mut().find(|q| q.id == id)
    }

    pub fn base(&self, q: u64, b: u64) -> Option<&BaseDoc> {
        self.quantity(q)?.bases.iter().find(|x| x.id == b)
    }

    pub fn base_mut(&mut self, q: u64, b: u64) -> Option<&mut BaseDoc> {
        self.quantity_mut(q)?.bases.iter_mut().find(|x| x.id == b)
    }

    // -----------------------------------------------------------------------
    // Reading
    // -----------------------------------------------------------------------

    pub fn from_yaml(text: &str) -> Result<Self, String> {
        if text.trim().is_empty() {
            return Ok(RulesDoc::default());
        }
        let v: Value = serde_norway::from_str(text).map_err(|e| e.to_string())?;
        let v = merged_keys(v);
        let Value::Mapping(top) = v else {
            return Err("a rule file is a mapping: version, quantities".into());
        };
        let mut doc = RulesDoc::default();
        for (k, v) in &top {
            match key(k)?.as_str() {
                "version" => {
                    if v.as_i64() != Some(1) {
                        return Err(format!("version: {} is not 1", text_of(v)));
                    }
                }
                "extends" => doc.extends = Some(v.clone()),
                "quantities" => {
                    let Value::Mapping(qs) = v else {
                        return Err("quantities: a mapping of name to quantity".into());
                    };
                    for (name, q) in qs {
                        let name = key(name)?;
                        doc.quantities.push(read_quantity(&name, q)?);
                    }
                }
                other => return Err(format!("unknown field '{other}'")),
            }
        }
        Ok(doc)
    }

    // -----------------------------------------------------------------------
    // Writing
    // -----------------------------------------------------------------------

    /// The file: block style down to an input, each term and each rule's
    /// `then` on one line, as rule files are written by hand.
    pub fn to_yaml(&self) -> Result<String, String> {
        let mut out = String::from("version: 1\n");
        if let Some(e) = &self.extends {
            let mut m = Mapping::new();
            m.insert(Value::String("extends".into()), e.clone());
            out.push_str(&serde_norway::to_string(&Value::Mapping(m)).map_err(|e| e.to_string())?);
        }
        if self.quantities.is_empty() {
            out.push_str("quantities: {}\n");
            return Ok(out);
        }
        out.push_str("quantities:\n");
        for q in &self.quantities {
            out.push_str(&format!("  {}:\n", scalar(&q.name)));
            if let Some(c) = &q.combine {
                out.push_str(&format!("    combine: {}\n", scalar(c)));
            }
            if q.bases.is_empty() {
                out.push_str("    rule_bases: {}\n");
                continue;
            }
            out.push_str("    rule_bases:\n");
            for b in &q.bases {
                out.push_str(&format!("      {}:\n", scalar(&b.name)));
                if b.inputs.is_none() && b.rules.is_none() {
                    out.push_str("        {}\n");
                }
                if let Some(inputs) = &b.inputs {
                    out.push_str("        inputs:\n");
                    for i in inputs {
                        out.push_str(&format!("          {}:\n", scalar(&i.name)));
                        out.push_str(&format!(
                            "            universe: [{}, {}]\n",
                            num(i.low),
                            num(i.high)
                        ));
                        if let Some(d) = i.default {
                            out.push_str(&format!("            default: {}\n", num(d)));
                        }
                        out.push_str("            terms:\n");
                        for t in &i.terms {
                            let ps: Vec<String> = t.shape.params().into_iter().map(num).collect();
                            out.push_str(&format!(
                                "              {}: {{{}: [{}]}}\n",
                                scalar(&t.name),
                                t.shape.kind(),
                                ps.join(", ")
                            ));
                        }
                    }
                }
                if let Some(rules) = &b.rules {
                    if rules.is_empty() {
                        out.push_str("        rules: []\n");
                    } else {
                        out.push_str("        rules:\n");
                    }
                    for r in rules {
                        let then: Vec<String> = b
                            .outputs
                            .iter()
                            .zip(&r.then)
                            .filter_map(|(o, v)| v.map(|v| format!("{}: {}", scalar(o), num(v))))
                            .collect();
                        out.push_str(&format!("          - if: {}\n", scalar(&r.when)));
                        out.push_str(&format!("            then: {{{}}}\n", then.join(", ")));
                    }
                }
            }
        }
        Ok(out)
    }
}

fn read_quantity(name: &str, v: &Value) -> Result<QuantityDoc, String> {
    let at = format!("quantities.{name}");
    let Value::Mapping(m) = v else {
        return Err(format!("{at}: a mapping (rule_bases, combine); a null removes it, which the form does not write: edit the YAML"));
    };
    let mut q = QuantityDoc {
        id: fresh_id(),
        name: name.to_string(),
        combine: None,
        bases: vec![],
    };
    for (k, v) in m {
        match key(k)?.as_str() {
            "combine" => q.combine = Some(text_of(v)),
            "rule_bases" => {
                let Value::Mapping(bs) = v else {
                    return Err(format!("{at}.rule_bases: a mapping of name to rule base"));
                };
                for (bn, b) in bs {
                    let bn = key(bn)?;
                    q.bases
                        .push(read_base(&bn, b, &format!("{at}.rule_bases.{bn}"))?);
                }
            }
            other => return Err(format!("{at}: unknown field '{other}'")),
        }
    }
    Ok(q)
}

fn read_base(name: &str, v: &Value, at: &str) -> Result<BaseDoc, String> {
    let empty = Mapping::new();
    let m = match v {
        Value::Mapping(m) => m,
        Value::Null => &empty,
        _ => return Err(format!("{at}: a mapping (inputs, rules)")),
    };
    let mut b = BaseDoc {
        id: fresh_id(),
        name: name.to_string(),
        inputs: None,
        outputs: vec![],
        rules: None,
    };
    for (k, v) in m {
        match key(k)?.as_str() {
            "inputs" => {
                let Value::Mapping(is) = v else {
                    return Err(format!("{at}.inputs: a mapping of name to input"));
                };
                let mut inputs = vec![];
                for (n, i) in is {
                    let n = key(n)?;
                    inputs.push(read_input(&n, i, &format!("{at}.inputs.{n}"))?);
                }
                b.inputs = Some(inputs);
            }
            "rules" => {
                let Value::Sequence(rs) = v else {
                    return Err(format!("{at}.rules: a list of rules"));
                };
                let mut rows: Vec<(String, Vec<(String, f64)>)> = vec![];
                for (n, r) in rs.iter().enumerate() {
                    let Value::Mapping(r) = r else {
                        return Err(format!("{at}.rules[{n}]: if and then"));
                    };
                    let mut when = String::new();
                    let mut then = vec![];
                    for (k, v) in r {
                        match key(k)?.as_str() {
                            "if" => when = text_of(v),
                            "then" => {
                                let Value::Mapping(t) = v else {
                                    return Err(format!("{at}.rules[{n}].then: output to value"));
                                };
                                for (o, x) in t {
                                    let o = key(o)?;
                                    let x = number(x).ok_or_else(|| {
                                        format!("{at}.rules[{n}].then.{o}: not a number")
                                    })?;
                                    if !b.outputs.contains(&o) {
                                        b.outputs.push(o.clone());
                                    }
                                    then.push((o, x));
                                }
                            }
                            other => {
                                return Err(format!("{at}.rules[{n}]: unknown field '{other}'"))
                            }
                        }
                    }
                    rows.push((when, then));
                }
                let outputs = b.outputs.clone();
                b.rules = Some(
                    rows.into_iter()
                        .map(|(when, then)| RuleDoc {
                            id: fresh_id(),
                            when,
                            then: outputs
                                .iter()
                                .map(|o| then.iter().find(|(n, _)| n == o).map(|(_, v)| *v))
                                .collect(),
                        })
                        .collect(),
                );
            }
            other => return Err(format!("{at}: unknown field '{other}'")),
        }
    }
    Ok(b)
}

fn read_input(name: &str, v: &Value, at: &str) -> Result<InputDoc, String> {
    let Value::Mapping(m) = v else {
        return Err(format!("{at}: a mapping (universe, terms, default)"));
    };
    let mut i = InputDoc {
        id: fresh_id(),
        name: name.to_string(),
        low: 0.0,
        high: 0.0,
        default: None,
        terms: vec![],
    };
    for (k, v) in m {
        match key(k)?.as_str() {
            "universe" => {
                let xs = numbers(v)
                    .filter(|xs| xs.len() == 2)
                    .ok_or_else(|| format!("{at}.universe: [low, high]"))?;
                i.low = xs[0];
                i.high = xs[1];
            }
            "default" => {
                i.default = Some(number(v).ok_or_else(|| format!("{at}.default: a number"))?)
            }
            "terms" => {
                let Value::Mapping(ts) = v else {
                    return Err(format!("{at}.terms: a mapping of name to shape"));
                };
                for (tn, t) in ts {
                    let tn = key(tn)?;
                    let tat = format!("{at}.terms.{tn}");
                    let Value::Mapping(t) = t else {
                        return Err(format!("{tat}: one of triangle, trapezoid, gaussian"));
                    };
                    let Some((kind, ps)) = t.iter().next() else {
                        return Err(format!("{tat}: one of triangle, trapezoid, gaussian"));
                    };
                    let ps = numbers(ps).ok_or_else(|| format!("{tat}: numbers"))?;
                    let shape = match (key(kind)?.as_str(), ps.len()) {
                        ("triangle", 3) => Shape::Triangle([ps[0], ps[1], ps[2]]),
                        ("trapezoid", 4) => Shape::Trapezoid([ps[0], ps[1], ps[2], ps[3]]),
                        ("gaussian", 2) => Shape::Gaussian {
                            mean: ps[0],
                            sigma: ps[1],
                        },
                        (k, n) => return Err(format!("{tat}: {k} with {n} numbers")),
                    };
                    i.terms.push(Term {
                        id: fresh_id(),
                        name: tn,
                        shape,
                    });
                }
            }
            other => return Err(format!("{at}: unknown field '{other}'")),
        }
    }
    Ok(i)
}

/// YAML merge keys (`<<: *anchor`) worked in, as the core reads them.
fn merged_keys(v: Value) -> Value {
    match v {
        Value::Mapping(m) => {
            let mut out = Mapping::new();
            let mut merged = Mapping::new();
            for (k, v) in m {
                if k.as_str() == Some("<<") {
                    let sources = match merged_keys(v) {
                        Value::Sequence(s) => s,
                        other => vec![other],
                    };
                    for s in sources {
                        if let Value::Mapping(s) = s {
                            for (k, v) in s {
                                if !merged.contains_key(&k) {
                                    merged.insert(k, v);
                                }
                            }
                        }
                    }
                } else {
                    out.insert(k, merged_keys(v));
                }
            }
            for (k, v) in merged {
                if !out.contains_key(&k) {
                    out.insert(k, v);
                }
            }
            Value::Mapping(out)
        }
        Value::Sequence(s) => Value::Sequence(s.into_iter().map(merged_keys).collect()),
        Value::Tagged(t) => merged_keys(t.value),
        other => other,
    }
}

fn key(v: &Value) -> Result<String, String> {
    match v {
        Value::String(s) => Ok(s.clone()),
        Value::Number(n) => Ok(n.to_string()),
        Value::Bool(b) => Ok(b.to_string()),
        other => Err(format!("a name, not {}", text_of(other))),
    }
}

fn text_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => "null".into(),
        other => serde_norway::to_string(other)
            .unwrap_or_default()
            .trim()
            .to_string(),
    }
}

fn number(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        _ => None,
    }
}

fn numbers(v: &Value) -> Option<Vec<f64>> {
    match v {
        Value::Sequence(s) => s.iter().map(number).collect(),
        _ => None,
    }
}

/// A number as a rule file writes it: `0`, `0.65`, `-6`.
pub fn num(v: f64) -> String {
    let v = tidy(v);
    if v == v.trunc() && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

/// A string as a plain scalar where YAML reads it back the same, else quoted.
fn scalar(s: &str) -> String {
    let plain = !s.is_empty()
        && serde_norway::from_str::<Value>(s).ok() == Some(Value::String(s.to_string()))
        && !s.contains(": ")
        && !s.contains(" #")
        && !s.starts_with([
            '{', '[', '&', '*', '!', '|', '>', '\'', '"', '%', '@', '`', '-', '?', ',', '#',
        ])
        && !s.ends_with(':');
    if plain {
        s.to_string()
    } else {
        serde_norway::to_string(&Value::String(s.to_string()))
            .map(|t| t.trim_end().to_string())
            .unwrap_or_else(|_| format!("{s:?}"))
    }
}

// ---------------------------------------------------------------------------
// Antecedents
// ---------------------------------------------------------------------------

/// The clauses of an antecedent that is `(input IS term)` clauses joined by
/// `AND`, each input once: what a row of term pickers shows. `None` for any
/// other antecedent (`OR`, `NOT`, nesting), which the form shows as text.
pub fn clauses(when: &str) -> Option<Vec<(String, String)>> {
    let mut s = when.trim();
    // One pair of parentheses around the whole conjunction.
    if s.starts_with("((") && s.ends_with("))") {
        s = &s[1..s.len() - 1];
    }
    let clause = regex::Regex::new(r"^\(\s*([^\s()]+)\s+IS\s+([^\s()]+)\s*\)$").unwrap();
    let mut out: Vec<(String, String)> = vec![];
    for part in s.split(" AND ") {
        let c = clause.captures(part.trim())?;
        let (i, t) = (c[1].to_string(), c[2].to_string());
        if i == "NOT" || t == "NOT" || out.iter().any(|(n, _)| *n == i) {
            return None;
        }
        out.push((i, t));
    }
    (!out.is_empty()).then_some(out)
}

/// The antecedent of `clauses`, joined by `AND`.
pub fn when_of(clauses: &[(String, String)]) -> String {
    clauses
        .iter()
        .map(|(i, t)| format!("({i} IS {t})"))
        .collect::<Vec<_>>()
        .join(" AND ")
}

// ---------------------------------------------------------------------------
// Checking
// ---------------------------------------------------------------------------

/// The files a document extends, as loaded: found by the label the core
/// gives them, or by their file name.
struct Library<'a>(&'a [(String, String)]);

impl Fetch for Library<'_> {
    fn fetch(&mut self, origin: &Origin) -> openodd::Result<String> {
        let label = origin.label();
        let base = label.rsplit('/').next().unwrap_or(&label).to_string();
        self.0
            .iter()
            .find(|(n, _)| *n == label)
            .or_else(|| self.0.iter().find(|(n, _)| *n == base))
            .map(|(_, t)| t.clone())
            .ok_or_else(|| {
                openodd::Error::value(format!(
                    "{label} is extended: load it under the file's settings"
                ))
            })
    }
}

/// openodd's reading of the rule file `text` named `name`, with the files it
/// extends from `library` (name, text).
pub fn check(text: &str, name: &str, library: &[(String, String)]) -> Result<RuleSet, String> {
    RuleSet::parse_with(text, name, &Origin::Text, &mut Library(library)).map_err(|e| e.to_string())
}

/// The values each input of `quantity` reads at `probe`: the probe's, else
/// the input's default, else the middle of its universe.
pub fn values_at(set: &RuleSet, quantity: &str, probe: &[(String, f64)]) -> IndexMap<String, f64> {
    let mut out = IndexMap::new();
    if let Some(q) = set.quantities.get(quantity) {
        for b in &q.rule_bases {
            for i in &b.inputs {
                if out.contains_key(&i.name) {
                    continue;
                }
                let v = probe
                    .iter()
                    .find(|(n, _)| *n == i.name)
                    .map(|(_, v)| *v)
                    .or(i.default)
                    .unwrap_or((i.low + i.high) / 2.0);
                out.insert(i.name.clone(), v);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = include_str!("../../../examples/wasm-viewer/sample_rules.yaml");

    #[test]
    fn a_rule_file_reads_and_writes_back_to_the_same_rules() {
        let doc = RulesDoc::from_yaml(SAMPLE).unwrap();
        assert_eq!(doc.quantities.len(), 2);
        let vis = &doc.quantities[1];
        assert_eq!(vis.combine.as_deref(), Some("min"));
        assert_eq!(vis.bases.len(), 3);
        let water = &doc.quantities[0].bases[0];
        assert_eq!(water.outputs, ["low", "high"]);
        assert_eq!(
            water.inputs.as_ref().unwrap()[0].terms[1].shape,
            Shape::Triangle([0.0, 50.0, 100.0])
        );
        let yaml = doc.to_yaml().unwrap();
        assert!(
            yaml.contains("              dry: {trapezoid: [0, 0, 0, 50]}\n"),
            "{yaml}"
        );
        assert!(yaml
            .contains("          - if: (water IS dry)\n            then: {low: 0.9, high: 1.1}\n"));
        let a = check(SAMPLE, "a.yaml", &[]).unwrap();
        let b = check(&yaml, "a.yaml", &[]).unwrap();
        assert_eq!(a.quantities, b.quantities);
        // And again: what it writes it reads the same.
        assert_eq!(RulesDoc::from_yaml(&yaml).unwrap().to_yaml().unwrap(), yaml);
    }

    #[test]
    fn simple_antecedents_are_term_pickers_and_others_text() {
        assert_eq!(
            clauses("(water IS wet) AND (sun IS night)"),
            Some(vec![
                ("water".into(), "wet".into()),
                ("sun".into(), "night".into())
            ])
        );
        assert_eq!(clauses("((a IS x) AND (b IS y))").map(|c| c.len()), Some(2));
        assert_eq!(clauses("(a IS x) OR (b IS y)"), None);
        assert_eq!(clauses("NOT (a IS x)"), None);
        assert_eq!(clauses("(a IS x) AND (a IS y)"), None);
        assert_eq!(when_of(&[("a".into(), "x".into())]), "(a IS x)");
    }

    #[test]
    fn renaming_an_input_or_a_term_renames_it_in_the_rules() {
        let mut doc = RulesDoc::from_yaml(SAMPLE).unwrap();
        let b = &mut doc.quantities[0].bases[0];
        b.rename_input("water", "road_water");
        b.rename_term("road_water", "wet", "damp");
        let rules = b.rules.as_ref().unwrap();
        assert_eq!(rules[0].when, "(road_water IS dry)");
        assert_eq!(rules[1].when, "(road_water IS damp)");
        assert!(check(&doc.to_yaml().unwrap(), "a.yaml", &[]).is_ok());
    }

    #[test]
    fn spread_terms_cover_the_universe_and_peak_where_neighbours_end() {
        let mut i = InputDoc::new("x".into());
        i.low = -90.0;
        i.high = 90.0;
        i.spread();
        let shapes: Vec<Shape> = i.terms.iter().map(|t| t.shape).collect();
        assert_eq!(
            shapes,
            [
                Shape::Trapezoid([-90.0, -90.0, -90.0, 0.0]),
                Shape::Triangle([-90.0, 0.0, 90.0]),
                Shape::Trapezoid([0.0, 90.0, 90.0, 90.0]),
            ]
        );
        let mut doc = RulesDoc::default();
        doc.quantities.push(QuantityDoc {
            id: fresh_id(),
            name: "q".into(),
            combine: None,
            bases: vec![BaseDoc::new("b".into(), "x".into(), &[])],
        });
        let set = check(&doc.to_yaml().unwrap(), "new.yaml", &[]).unwrap();
        assert_eq!(set.quantities["q"].outputs, ["low", "high"]);
    }

    #[test]
    fn dragging_keeps_corners_in_order() {
        let s = Shape::Triangle([0.0, 50.0, 100.0]);
        assert_eq!(
            s.drag(1, 120.0, 0.0, 100.0),
            Shape::Triangle([0.0, 100.0, 100.0])
        );
        assert_eq!(
            s.drag(0, 70.0, 0.0, 100.0),
            Shape::Triangle([50.0, 50.0, 100.0])
        );
        let g = Shape::Gaussian {
            mean: 50.0,
            sigma: 10.0,
        };
        assert_eq!(
            g.drag(1, 30.0, 0.0, 100.0),
            Shape::Gaussian {
                mean: 50.0,
                sigma: 20.0
            }
        );
    }

    #[test]
    fn a_file_extending_another_reads_with_it_loaded() {
        let over = "version: 1\nextends: base.yaml\nquantities:\n  tire_friction:\n    rule_bases:\n      water:\n        rules:\n          - {if: (water IS dry), then: {low: 0.95, high: 1.05}}\n          - {if: (water IS wet), then: {low: 0.6, high: 0.8}}\n          - {if: (water IS flooded), then: {low: 0.3, high: 0.5}}\n";
        let doc = RulesDoc::from_yaml(over).unwrap();
        let b = &doc.quantities[0].bases[0];
        assert!(b.inputs.is_none());
        assert_eq!(b.rules.as_ref().unwrap().len(), 3);
        let yaml = doc.to_yaml().unwrap();
        assert!(yaml.contains("extends: base.yaml"));
        assert!(check(&yaml, "mini.yaml", &[])
            .unwrap_err()
            .contains("base.yaml"));
        let set = check(&yaml, "mini.yaml", &[("base.yaml".into(), SAMPLE.into())]).unwrap();
        let mut at = IndexMap::new();
        at.insert("water".to_string(), 100.0);
        assert_eq!(set.evaluate("tire_friction", &at).unwrap()["low"], 0.3);
    }

    #[test]
    fn names_that_need_quotes_are_quoted() {
        assert_eq!(scalar("low"), "low");
        assert_eq!(scalar("(water IS dry)"), "(water IS dry)");
        assert_eq!(scalar("1"), "'1'");
        assert_eq!(num(0.65), "0.65");
        assert_eq!(num(-6.0), "-6");
        assert_eq!(num(0.1 + 0.2), "0.3");
    }
}
