//! A rule file ([docs/rules.md](../../../docs/rules.md)) as the rules editor
//! holds it, read from YAML and written back, and checked by openodd's own
//! reader.
//!
//! The form edits the fields a rule file has: the inputs it describes, the
//! quantities, each quantity's parts, and each part's points (`at`) and the
//! distribution written at them. A file that `extends` others may leave a
//! part's points or its distribution to them: the form keeps what the file
//! does not name absent ([`PartDoc::axes`], [`PartDoc::dist`]). Comments are
//! not kept.

use indexmap::IndexMap;
use openodd::rules::{Axis, Dist, Family, Fetch, Origin, Part, RuleSet};
use serde_norway::{Mapping, Value};

use crate::doc::fresh_id;

/// An input of a part, and the points its distribution is written at.
#[derive(Clone, Debug, PartialEq)]
pub struct AxisDoc {
    pub id: u64,
    pub input: String,
    /// Increasing.
    pub points: Vec<f64>,
}

/// One parameter of a distribution: a value per point, the last axis
/// fastest, or one number for all (`constant`).
#[derive(Clone, Debug, PartialEq)]
pub struct ParamDoc {
    pub name: String,
    pub values: Vec<f64>,
    pub constant: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DistDoc {
    pub family: Family,
    /// In [`Family::params`]' order.
    pub params: Vec<ParamDoc>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PartDoc {
    pub id: u64,
    pub name: String,
    /// `None`: the file does not name them (an extended file's are read).
    pub axes: Option<Vec<AxisDoc>>,
    /// `None`: the file does not name it.
    pub dist: Option<DistDoc>,
    pub text: String,
}

impl PartDoc {
    /// A part over `input`, uniform between 0 and 1 at three points.
    pub fn new(name: String, input: String) -> Self {
        let points = vec![0.0, 50.0, 100.0];
        PartDoc {
            id: fresh_id(),
            name,
            axes: Some(vec![AxisDoc {
                id: fresh_id(),
                input,
                points,
            }]),
            dist: Some(DistDoc {
                family: Family::Uniform,
                params: vec![param("low", vec![0.0; 3]), param("high", vec![1.0; 3])],
            }),
            text: String::new(),
        }
    }

    /// How many combinations of points it writes.
    pub fn size(&self) -> usize {
        self.axes
            .as_ref()
            .map(|a| a.iter().map(|x| x.points.len()).product())
            .unwrap_or(0)
    }

    /// As openodd holds it, when the file names both its points and its
    /// distribution and they agree in size.
    pub fn core(&self) -> Option<Part> {
        let axes = self.axes.as_ref()?;
        let dist = self.dist.as_ref()?;
        let size = self.size();
        if dist.params.iter().any(|p| p.values.len() != size) {
            return None;
        }
        Some(Part {
            name: self.name.clone(),
            axes: axes
                .iter()
                .map(|a| Axis {
                    input: a.input.clone(),
                    points: a.points.clone(),
                })
                .collect(),
            family: dist.family,
            values: dist.params.iter().map(|p| p.values.clone()).collect(),
            text: self.text.clone(),
        })
    }

    /// Replace axis `k`'s points with `points` (increasing), the
    /// distribution's values read at the new points from the old ones.
    pub fn set_points(&mut self, k: usize, points: Vec<f64>) {
        let old = self.core();
        let Some(axes) = &mut self.axes else { return };
        let Some(axis) = axes.get_mut(k) else { return };
        axis.points = points;
        let grid = grid(axes);
        if let (Some(old), Some(dist)) = (old, &mut self.dist) {
            let fresh: Vec<Dist> = grid.iter().map(|at| old.dist(at)).collect();
            for (j, p) in dist.params.iter_mut().enumerate() {
                p.values = fresh.iter().map(|d| tidy(d.params()[j])).collect();
            }
        }
    }

    /// The same part with a distribution of `family`, each point's made from
    /// the one there ([`convert`]).
    pub fn set_family(&mut self, family: Family) {
        let size = self.size();
        let Some(dist) = &mut self.dist else { return };
        if dist.family == family {
            return;
        }
        let at = |k: usize| -> Dist {
            let p: Vec<f64> = dist.params.iter().map(|p| p.values[k]).collect();
            dist.family.dist(&p)
        };
        let converted: Vec<Vec<f64>> = (0..size).map(|k| convert(&at(k), family)).collect();
        dist.family = family;
        dist.params = family
            .params()
            .iter()
            .enumerate()
            .map(|(j, n)| {
                let values: Vec<f64> = converted.iter().map(|c| tidy(c[j])).collect();
                let constant = values.windows(2).all(|w| w[0] == w[1]) && size > 1;
                ParamDoc {
                    name: n.to_string(),
                    values,
                    constant,
                }
            })
            .collect();
    }
}

impl PartDoc {
    /// Read `input` too, at `points` (increasing): a new last axis, the
    /// distribution the same along it.
    pub fn add_axis(&mut self, input: String, points: Vec<f64>) {
        let old = self.core();
        let Some(axes) = &mut self.axes else { return };
        axes.push(AxisDoc {
            id: fresh_id(),
            input,
            points,
        });
        let grid = grid(axes);
        if let (Some(old), Some(dist)) = (old, &mut self.dist) {
            let fresh: Vec<Dist> = grid
                .iter()
                .map(|at| old.dist(&at[..at.len() - 1]))
                .collect();
            for (j, p) in dist.params.iter_mut().enumerate() {
                p.values = fresh.iter().map(|d| tidy(d.params()[j])).collect();
            }
        }
    }

    /// Stop reading axis `k`'s input: the distribution is the one at its
    /// first point. A part's last axis stays.
    pub fn remove_axis(&mut self, k: usize) -> bool {
        let old = self.core();
        let Some(axes) = &mut self.axes else {
            return false;
        };
        if axes.len() < 2 || k >= axes.len() {
            return false;
        }
        let at_k = axes[k].points[0];
        axes.remove(k);
        let grid = grid(axes);
        if let (Some(old), Some(dist)) = (old, &mut self.dist) {
            let fresh: Vec<Dist> = grid
                .iter()
                .map(|at| {
                    let mut full = at.clone();
                    full.insert(k, at_k);
                    old.dist(&full)
                })
                .collect();
            for (j, p) in dist.params.iter_mut().enumerate() {
                p.values = fresh.iter().map(|d| tidy(d.params()[j])).collect();
            }
        }
        true
    }

    /// The inputs its axes read, in order.
    pub fn reads(&self) -> Vec<String> {
        self.axes
            .as_ref()
            .map(|a| a.iter().map(|x| x.input.clone()).collect())
            .unwrap_or_default()
    }
}

impl RulesDoc {
    /// Rename the quantity `old` to `new`, and every part reading it.
    pub fn rename_quantity(&mut self, old: &str, new: &str) {
        for q in &mut self.quantities {
            if q.name == old {
                q.name = new.to_string();
            }
        }
        self.rename_reads(old, new);
    }

    /// Rename the input `old` to `new`: the file's description of it and
    /// every axis reading it.
    pub fn rename_input(&mut self, old: &str, new: &str) {
        for i in &mut self.inputs {
            if i.name == old {
                i.name = new.to_string();
            }
        }
        self.rename_reads(old, new);
    }

    fn rename_reads(&mut self, old: &str, new: &str) {
        for q in &mut self.quantities {
            for p in &mut q.parts {
                for a in p.axes.iter_mut().flatten() {
                    if a.input == old {
                        a.input = new.to_string();
                    }
                }
            }
        }
    }

    /// Whether `to` is `from` or reads it, through any chain of quantities:
    /// so a part of `from` reading `to` would close a cycle.
    pub fn reaches(&self, from: &str, to: &str) -> bool {
        let mut stack = vec![from.to_string()];
        let mut seen: Vec<String> = Vec::new();
        while let Some(n) = stack.pop() {
            if n == to {
                return true;
            }
            if seen.contains(&n) {
                continue;
            }
            seen.push(n.clone());
            for q in &self.quantities {
                if q.parts.iter().any(|p| p.reads().contains(&n)) {
                    stack.push(q.name.clone());
                }
            }
        }
        false
    }

    /// The inputs the parts read that are not quantities, and the ones the
    /// file describes, each once.
    pub fn free_inputs(&self) -> Vec<String> {
        let names: Vec<&str> = self.quantities.iter().map(|q| q.name.as_str()).collect();
        let mut out: Vec<String> = Vec::new();
        for q in &self.quantities {
            for p in &q.parts {
                for i in p.reads() {
                    if !names.contains(&i.as_str()) && !out.contains(&i) {
                        out.push(i);
                    }
                }
            }
        }
        for i in &self.inputs {
            if !names.contains(&i.name.as_str()) && !out.contains(&i.name) {
                out.push(i.name.clone());
            }
        }
        out
    }
}

fn param(name: &str, values: Vec<f64>) -> ParamDoc {
    ParamDoc {
        name: name.to_string(),
        values,
        constant: false,
    }
}

/// Every combination of the axes' points, the last axis fastest.
pub fn grid(axes: &[AxisDoc]) -> Vec<Vec<f64>> {
    let mut out: Vec<Vec<f64>> = vec![vec![]];
    for a in axes {
        out = out
            .into_iter()
            .flat_map(|prefix| {
                a.points.iter().map(move |p| {
                    let mut v = prefix.clone();
                    v.push(*p);
                    v
                })
            })
            .collect();
    }
    out
}

/// A distribution's parameters as another family's, over about the same
/// values: a uniform's ends become a triangle's (its mode the middle) or a
/// truncated normal's (its sd a quarter of the span).
pub fn convert(d: &Dist, to: Family) -> Vec<f64> {
    let (low, high) = d.support();
    let centre = match *d {
        Dist::Uniform { low, high } => (low + high) / 2.0,
        Dist::Triangular { mode, .. } => mode,
        Dist::Normal { mean, .. } => mean.clamp(low, high),
    };
    match to {
        Family::Uniform => vec![low, high],
        Family::Triangular => vec![low, centre, high],
        Family::Normal => {
            let sd = match *d {
                Dist::Normal { sd, .. } => sd,
                _ => ((high - low) / 4.0).max(f64::EPSILON),
            };
            vec![centre, sd, low, high]
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
pub struct QuantityDoc {
    pub id: u64,
    pub name: String,
    /// `None`: not written (`min`, unless an extended file says otherwise).
    pub combine: Option<String>,
    pub parts: Vec<PartDoc>,
    pub text: String,
}

/// An input the file describes.
#[derive(Clone, Debug, PartialEq)]
pub struct InputDoc {
    pub id: u64,
    pub name: String,
    pub default: Option<f64>,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct RulesDoc {
    /// What the file `extends`, as written.
    pub extends: Option<Value>,
    pub inputs: Vec<InputDoc>,
    pub quantities: Vec<QuantityDoc>,
}

impl RulesDoc {
    pub fn quantity(&self, id: u64) -> Option<&QuantityDoc> {
        self.quantities.iter().find(|q| q.id == id)
    }

    pub fn quantity_mut(&mut self, id: u64) -> Option<&mut QuantityDoc> {
        self.quantities.iter_mut().find(|q| q.id == id)
    }

    pub fn part(&self, q: u64, p: u64) -> Option<&PartDoc> {
        self.quantity(q)?.parts.iter().find(|x| x.id == p)
    }

    pub fn part_mut(&mut self, q: u64, p: u64) -> Option<&mut PartDoc> {
        self.quantity_mut(q)?.parts.iter_mut().find(|x| x.id == p)
    }

    // -----------------------------------------------------------------------
    // Reading
    // -----------------------------------------------------------------------

    pub fn from_yaml(text: &str) -> Result<Self, String> {
        if text.trim().is_empty() {
            return Ok(RulesDoc::default());
        }
        let v: Value = serde_norway::from_str(text).map_err(|e| e.to_string())?;
        let Value::Mapping(top) = merged_keys(v) else {
            return Err("a rule file is a mapping: version, quantities".into());
        };
        let mut doc = RulesDoc::default();
        for (k, v) in &top {
            match key(k)?.as_str() {
                "version" => match v.as_i64() {
                    Some(2) => {}
                    Some(1) => {
                        return Err(
                            "version 1 (fuzzy rules) is no longer read: write each rule \
                                    base as a part, its rules as the values at its inputs' \
                                    points (docs/rules.md, \"From version 1\")"
                                .into(),
                        )
                    }
                    _ => return Err(format!("version: {} is not 2", text_of(v))),
                },
                "extends" => doc.extends = Some(v.clone()),
                "inputs" => {
                    for (n, i) in mapping(v, "inputs")? {
                        let n = key(n)?;
                        let at = format!("inputs.{n}");
                        let mut input = InputDoc {
                            id: fresh_id(),
                            name: n,
                            default: None,
                            text: String::new(),
                        };
                        for (f, x) in mapping(i, &at)? {
                            match key(f)?.as_str() {
                                "default" => {
                                    input.default = Some(
                                        number(x)
                                            .ok_or_else(|| format!("{at}.default: a number"))?,
                                    )
                                }
                                "text" => input.text = text_of(x),
                                other => return Err(format!("{at}: unknown field '{other}'")),
                            }
                        }
                        doc.inputs.push(input);
                    }
                }
                "quantities" => {
                    for (name, q) in mapping(v, "quantities")? {
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

    /// The file: block style down to a part, each parameter's values on one
    /// line, as rule files are written by hand.
    pub fn to_yaml(&self) -> Result<String, String> {
        let mut out = String::from("version: 2\n");
        if let Some(e) = &self.extends {
            let mut m = Mapping::new();
            m.insert(Value::String("extends".into()), e.clone());
            out.push_str(&serde_norway::to_string(&Value::Mapping(m)).map_err(|e| e.to_string())?);
        }
        if !self.inputs.is_empty() {
            out.push_str("inputs:\n");
            for i in &self.inputs {
                let mut fields = vec![];
                if let Some(d) = i.default {
                    fields.push(format!("default: {}", num(d)));
                }
                if !i.text.is_empty() {
                    fields.push(format!("text: {}", scalar(&i.text)));
                }
                out.push_str(&format!(
                    "  {}: {{{}}}\n",
                    scalar(&i.name),
                    fields.join(", ")
                ));
            }
        }
        if self.quantities.is_empty() {
            out.push_str("quantities: {}\n");
            return Ok(out);
        }
        out.push_str("quantities:\n");
        for q in &self.quantities {
            out.push_str(&format!("  {}:\n", scalar(&q.name)));
            if !q.text.is_empty() {
                out.push_str(&format!("    text: {}\n", scalar(&q.text)));
            }
            if let Some(c) = &q.combine {
                out.push_str(&format!("    combine: {}\n", scalar(c)));
            }
            if q.parts.is_empty() {
                out.push_str("    parts: {}\n");
                continue;
            }
            out.push_str("    parts:\n");
            for p in &q.parts {
                out.push_str(&format!("      {}:\n", scalar(&p.name)));
                if p.axes.is_none() && p.dist.is_none() && p.text.is_empty() {
                    out.push_str("        {}\n");
                }
                if !p.text.is_empty() {
                    out.push_str(&format!("        text: {}\n", scalar(&p.text)));
                }
                if let Some(axes) = &p.axes {
                    let at: Vec<String> = axes
                        .iter()
                        .map(|a| format!("{}: {}", scalar(&a.input), list(&a.points)))
                        .collect();
                    out.push_str(&format!("        at: {{{}}}\n", at.join(", ")));
                }
                if let Some(d) = &p.dist {
                    out.push_str(&format!("        {}:\n", d.family.name()));
                    let shape: Vec<usize> = p
                        .axes
                        .as_ref()
                        .map(|a| a.iter().map(|x| x.points.len()).collect())
                        .unwrap_or_else(|| vec![d.params.first().map_or(0, |x| x.values.len())]);
                    for param in &d.params {
                        let all_same = param.values.windows(2).all(|w| w[0] == w[1]);
                        let written = if (param.constant || param.values.len() == 1) && all_same {
                            param
                                .values
                                .first()
                                .map(|v| num(*v))
                                .unwrap_or_else(|| "0".into())
                        } else {
                            nested(&param.values, &shape)
                        };
                        out.push_str(&format!("          {}: {written}\n", param.name));
                    }
                }
            }
        }
        Ok(out)
    }
}

fn list(xs: &[f64]) -> String {
    format!(
        "[{}]",
        xs.iter().map(|x| num(*x)).collect::<Vec<_>>().join(", ")
    )
}

/// `values` (the last axis fastest) as nested lists of `shape`.
fn nested(values: &[f64], shape: &[usize]) -> String {
    match shape {
        [] | [_] => list(values),
        [_, rest @ ..] => {
            let step: usize = rest.iter().product();
            if step == 0 {
                return "[]".into();
            }
            let rows: Vec<String> = values.chunks(step).map(|c| nested(c, rest)).collect();
            format!("[{}]", rows.join(", "))
        }
    }
}

fn read_quantity(name: &str, v: &Value) -> Result<QuantityDoc, String> {
    let at = format!("quantities.{name}");
    let mut q = QuantityDoc {
        id: fresh_id(),
        name: name.to_string(),
        combine: None,
        parts: vec![],
        text: String::new(),
    };
    for (k, v) in mapping(v, &at)? {
        match key(k)?.as_str() {
            "combine" => q.combine = Some(text_of(v)),
            "text" => q.text = text_of(v),
            "parts" => {
                for (pn, p) in mapping(v, &format!("{at}.parts"))? {
                    let pn = key(pn)?;
                    q.parts
                        .push(read_part(&pn, p, &format!("{at}.parts.{pn}"))?);
                }
            }
            other => return Err(format!("{at}: unknown field '{other}'")),
        }
    }
    Ok(q)
}

fn read_part(name: &str, v: &Value, at: &str) -> Result<PartDoc, String> {
    let empty = Mapping::new();
    let m = match v {
        Value::Mapping(m) => m,
        Value::Null => &empty,
        _ => return Err(format!("{at}: a mapping (at, a distribution)")),
    };
    let mut p = PartDoc {
        id: fresh_id(),
        name: name.to_string(),
        axes: None,
        dist: None,
        text: String::new(),
    };
    let mut raw_dist: Option<(Family, &Value)> = None;
    for (k, v) in m {
        let k = key(k)?;
        if let Some(f) = Family::from_name(&k) {
            if raw_dist.is_some() {
                return Err(format!("{at}: more than one distribution"));
            }
            raw_dist = Some((f, v));
            continue;
        }
        match k.as_str() {
            "at" => {
                let mut axes = vec![];
                for (input, points) in mapping(v, &format!("{at}.at"))? {
                    let input = key(input)?;
                    let points = match points {
                        Value::Sequence(_) => numbers(points),
                        other => number(other).map(|x| vec![x]),
                    }
                    .ok_or_else(|| format!("{at}.at.{input}: numbers"))?;
                    axes.push(AxisDoc {
                        id: fresh_id(),
                        input,
                        points,
                    });
                }
                p.axes = Some(axes);
            }
            "text" => p.text = text_of(v),
            other => return Err(format!("{at}: unknown field '{other}'")),
        }
    }
    if let Some((family, v)) = raw_dist {
        let here = format!("{at}.{}", family.name());
        let fields = mapping(v, &here)?;
        let size = p.size();
        let mut params = vec![];
        for n in family.params() {
            let raw = fields
                .iter()
                .find(|(k, _)| k.as_str() == Some(*n))
                .map(|(_, v)| v);
            let (values, constant) = match raw {
                None => (vec![0.0; size.max(1)], true),
                Some(Value::Number(_)) => {
                    let x = number(raw.unwrap()).unwrap_or(0.0);
                    (vec![x; size.max(1)], true)
                }
                Some(v) => (
                    flatten(v).ok_or_else(|| format!("{here}.{n}: numbers"))?,
                    false,
                ),
            };
            params.push(ParamDoc {
                name: n.to_string(),
                values,
                constant,
            });
        }
        p.dist = Some(DistDoc { family, params });
    }
    Ok(p)
}

fn flatten(v: &Value) -> Option<Vec<f64>> {
    match v {
        Value::Sequence(items) => {
            let mut out = vec![];
            for i in items {
                out.extend(flatten(i)?);
            }
            Some(out)
        }
        other => number(other).map(|x| vec![x]),
    }
}

fn mapping<'a>(v: &'a Value, at: &str) -> Result<&'a Mapping, String> {
    match v {
        Value::Mapping(m) => Ok(m),
        _ => Err(format!("{at}: a mapping")),
    }
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

/// A computed number for display: four significant digits (`397.3`).
pub fn short(v: f64) -> String {
    if !v.is_finite() || v == 0.0 {
        return num(v);
    }
    let digits = 3 - v.abs().log10().floor() as i32;
    let p = 10f64.powi(digits.clamp(0, 12));
    num((v * p).round() / p)
}

/// A string as a plain scalar where YAML reads it back the same, else quoted.
fn scalar(s: &str) -> String {
    let plain = !s.is_empty()
        && serde_norway::from_str::<Value>(s).ok() == Some(Value::String(s.to_string()))
        && !s.contains(": ")
        && !s.contains(" #")
        && !s.contains(',')
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

/// The range of an input over every part of `quantity` reading it: the
/// outermost points.
pub fn input_range(set: &RuleSet, quantity: &str, input: &str) -> Option<(f64, f64)> {
    let q = set.quantities.get(quantity)?;
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    for p in &q.parts {
        for a in p.axes.iter().filter(|a| a.input == input) {
            lo = lo.min(a.points[0]);
            hi = hi.max(*a.points.last().unwrap());
        }
    }
    if lo > hi {
        return None;
    }
    if lo == hi {
        return Some((lo - 1.0, hi + 1.0));
    }
    Some((lo, hi))
}

/// The values each input of `quantity` reads at `probe`: the probe's, else
/// the file's default, else the middle of its points.
pub fn values_at(set: &RuleSet, quantity: &str, probe: &[(String, f64)]) -> IndexMap<String, f64> {
    let mut out = IndexMap::new();
    let Some(q) = set.quantities.get(quantity) else {
        return out;
    };
    for name in q.inputs() {
        // A quantity read is worked out from its own inputs unless the probe
        // names a value for it (observed).
        if set.quantities.contains_key(&name) && !probe.iter().any(|(n, _)| *n == name) {
            continue;
        }
        let v = probe
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, v)| *v)
            .or_else(|| set.inputs.get(&name).and_then(|i| i.default))
            .or_else(|| input_range(set, quantity, &name).map(|(a, b)| (a + b) / 2.0))
            .unwrap_or(0.0);
        out.insert(name, v);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = include_str!("../../../examples/wasm-viewer/sample_rules.yaml");

    const CHAIN: &str = include_str!("../../../examples/wasm-viewer/chain_rules.yaml");

    #[test]
    fn an_axis_added_keeps_the_distribution_and_one_removed_takes_its_first_point() {
        let mut doc = RulesDoc::from_yaml(SAMPLE).unwrap();
        let part = &mut doc.quantities[0].parts[0];
        part.add_axis("sun_altitude_angle".into(), vec![-6.0, 0.0]);
        assert_eq!(part.reads(), ["water", "sun_altitude_angle"]);
        let low = &part.dist.as_ref().unwrap().params[0].values;
        // Each old value along the new axis, the last fastest.
        assert_eq!(low, &[0.9, 0.9, 0.65, 0.65, 0.45, 0.45]);
        assert!(check(&doc.to_yaml().unwrap(), "a.yaml", &[]).is_ok());
        let part = &mut doc.quantities[0].parts[0];
        assert!(part.remove_axis(0));
        assert_eq!(part.reads(), ["sun_altitude_angle"]);
        // At water's first point, 0: the dry road's.
        assert_eq!(part.dist.as_ref().unwrap().params[0].values, [0.9, 0.9]);
        assert!(!part.remove_axis(0), "a part keeps its last axis");
    }

    #[test]
    fn renaming_follows_every_reader_and_reaches_finds_cycles() {
        let mut doc = RulesDoc::from_yaml(CHAIN).unwrap();
        assert!(doc.reaches("visibility", "sight_speed"));
        assert!(doc.reaches("visibility", "visibility"));
        assert!(!doc.reaches("sight_speed", "visibility"));
        assert!(!doc.free_inputs().contains(&"visibility".to_string()));
        doc.rename_quantity("visibility", "sight");
        doc.rename_input("fog_density", "fog");
        let yaml = doc.to_yaml().unwrap();
        assert!(
            yaml.contains("at: {sight: [15, 50, 100, 250, 400]}"),
            "{yaml}"
        );
        assert!(yaml.contains("at: {fog: [0, 30, 70, 100]}"), "{yaml}");
        let set = check(&yaml, "a.yaml", &[]).unwrap();
        assert_eq!(set.upstream("sight_speed").unwrap(), ["sight"]);
    }

    #[test]
    fn a_rule_file_reads_and_writes_back_to_the_same_rules() {
        let doc = RulesDoc::from_yaml(SAMPLE).unwrap();
        assert_eq!(doc.quantities.len(), 2);
        let vis = &doc.quantities[1];
        assert_eq!(vis.combine.as_deref(), Some("min"));
        assert_eq!(vis.parts.len(), 3);
        let water = &doc.quantities[0].parts[0];
        assert_eq!(water.axes.as_ref().unwrap()[0].points, [0.0, 50.0, 100.0]);
        assert_eq!(
            water.dist.as_ref().unwrap().params[0].values,
            [0.9, 0.65, 0.45]
        );
        let yaml = doc.to_yaml().unwrap();
        assert!(yaml.contains("        at: {water: [0, 50, 100]}\n        uniform:\n          low: [0.9, 0.65, 0.45]\n"), "{yaml}");
        let a = check(SAMPLE, "a.yaml", &[]).unwrap();
        let b = check(&yaml, "a.yaml", &[]).unwrap();
        assert_eq!(a.quantities, b.quantities);
        assert_eq!(RulesDoc::from_yaml(&yaml).unwrap().to_yaml().unwrap(), yaml);
    }

    #[test]
    fn several_inputs_are_nested_and_a_constant_stays_one_number() {
        let text = "version: 2\ninputs:\n  b: {default: 1, text: the b}\nquantities:\n  q:\n    parts:\n      p:\n        at: {a: [0, 10], b: [0, 1]}\n        normal:\n          mean: [[0, 10], [20, 30]]\n          sd: 2\n          low: -50\n          high: 80\n";
        let doc = RulesDoc::from_yaml(text).unwrap();
        let p = &doc.quantities[0].parts[0];
        assert_eq!(p.size(), 4);
        assert_eq!(
            p.dist.as_ref().unwrap().params[0].values,
            [0.0, 10.0, 20.0, 30.0]
        );
        let yaml = doc.to_yaml().unwrap();
        assert!(
            yaml.contains("mean: [[0, 10], [20, 30]]\n          sd: 2\n"),
            "{yaml}"
        );
        assert!(yaml.contains("  b: {default: 1, text: the b}\n"), "{yaml}");
        let set = check(&yaml, "q.yaml", &[]).unwrap();
        assert_eq!(set.inputs["b"].default, Some(1.0));
    }

    #[test]
    fn new_points_read_the_values_from_the_old_ones() {
        let mut doc = RulesDoc::from_yaml(SAMPLE).unwrap();
        let p = &mut doc.quantities[0].parts[0];
        p.set_points(0, vec![0.0, 25.0, 100.0]);
        let d = p.dist.as_ref().unwrap();
        assert_eq!(d.params[0].values, [0.9, 0.775, 0.45]);
        assert_eq!(d.params[1].values, [1.1, 0.975, 0.65]);
    }

    #[test]
    fn a_family_is_made_from_the_one_there() {
        let mut p = PartDoc::new("p".into(), "x".into());
        if let Some(d) = &mut p.dist {
            d.params[0].values = vec![0.0, 2.0, 4.0];
            d.params[1].values = vec![4.0, 6.0, 8.0];
        }
        p.set_family(Family::Triangular);
        let d = p.dist.as_ref().unwrap();
        assert_eq!(d.params[1].name, "mode");
        assert_eq!(d.params[1].values, [2.0, 4.0, 6.0]);
        p.set_family(Family::Normal);
        let d = p.dist.as_ref().unwrap();
        assert_eq!(
            d.params.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(),
            ["mean", "sd", "low", "high"]
        );
        assert_eq!(d.params[1].values, [1.0, 1.0, 1.0]);
        let mut doc = RulesDoc::default();
        doc.quantities.push(QuantityDoc {
            id: fresh_id(),
            name: "q".into(),
            combine: None,
            parts: vec![p],
            text: String::new(),
        });
        assert!(check(&doc.to_yaml().unwrap(), "q.yaml", &[]).is_ok());
    }

    #[test]
    fn a_file_extending_another_reads_with_it_loaded() {
        let over = "version: 2\nextends: base.yaml\nquantities:\n  tire_friction:\n    parts:\n      water:\n        uniform: {low: [0.95, 0.6, 0.3], high: [1.05, 0.8, 0.5]}\n";
        let doc = RulesDoc::from_yaml(over).unwrap();
        let p = &doc.quantities[0].parts[0];
        assert!(p.axes.is_none());
        assert_eq!(p.dist.as_ref().unwrap().params[0].values, [0.95, 0.6, 0.3]);
        let yaml = doc.to_yaml().unwrap();
        assert!(yaml.contains("extends: base.yaml"));
        assert!(check(&yaml, "mini.yaml", &[])
            .unwrap_err()
            .contains("base.yaml"));
        let set = check(&yaml, "mini.yaml", &[("base.yaml".into(), SAMPLE.into())]).unwrap();
        let at = values_at(&set, "tire_friction", &[("water".into(), 100.0)]);
        assert_eq!(
            set.quantity("tire_friction")
                .unwrap()
                .joined(&at, &set.inputs)
                .unwrap()
                .support(),
            (0.3, 0.5)
        );
    }

    #[test]
    fn version_one_is_refused_with_the_way_on() {
        let e = RulesDoc::from_yaml("version: 1\nquantities: {}\n").unwrap_err();
        assert!(e.contains("From version 1"), "{e}");
    }

    #[test]
    fn names_that_need_quotes_are_quoted() {
        assert_eq!(scalar("low"), "low");
        assert_eq!(scalar("1"), "'1'");
        assert_eq!(num(0.65), "0.65");
        assert_eq!(num(-6.0), "-6");
        assert_eq!(num(0.1 + 0.2), "0.3");
    }
}
