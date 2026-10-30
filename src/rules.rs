//! Fuzzy rules: how a backend derives what a run sets from what it was given.
//!
//! A backend's knobs set what an ODD names -- the rain, the sun. What those do
//! to the rest of a run -- the road's grip, how far drivers see -- is the
//! backend's own model, which it may write as rules: a [`RuleSet`], read from
//! YAML.
//!
//! A rule set holds **quantities**, each one or more **rule bases**. A rule
//! base reads some inputs, sorts each into fuzzy terms over its universe, and
//! its rules give, per combination of terms, a value of each of its outputs:
//!
//! ```yaml
//! version: 1
//! quantities:
//!   tire_friction:
//!     rule_bases:
//!       water:
//!         inputs:
//!           water:
//!             universe: [0, 100]
//!             terms:
//!               dry: {trapezoid: [0, 0, 0, 50]}
//!               wet: {triangle: [0, 50, 100]}
//!               flooded: {trapezoid: [50, 100, 100, 100]}
//!         rules:
//!           - if: (water IS dry)
//!             then: {low: 0.90, high: 1.10}
//!           - if: (water IS wet)
//!             then: {low: 0.65, high: 0.85}
//!           - if: (water IS flooded)
//!             then: {low: 0.45, high: 0.65}
//! ```
//!
//! Inference is Sugeno's, zero order: an output is the rules' values of it,
//! averaged by how strongly each rule fires. A rule fires as its antecedent
//! holds: `AND` the least of its clauses, `OR` the greatest, `NOT` one less
//! it -- simpful's operators, so a rule file reads the same in either. With
//! more than one rule base, `combine` (`min`, the default, or `max`) joins
//! their outputs, output by output.
//!
//! A rule set is checked when read: every term and rule well formed, every
//! input's universe covered by its terms, and some rule firing everywhere on a
//! grid over the inputs' universes. A value outside an input's universe is
//! clamped to it. Which inputs exist is the backend's to say
//! ([`RuleSet::check_inputs`]); one an input names a `default` for may be left
//! out.

use indexmap::IndexMap;

use crate::error::{Error, Result};
use crate::value::py_float_repr;
use crate::yaml::{parse_all, Node, NodeExt};

/// The rule file format this module reads.
pub const VERSION: i64 = 1;
/// The points per input the grid check samples, at most, and in all.
const CHECK_POINTS: usize = 201;
const CHECK_TOTAL: f64 = 20000.0;

/// A fuzzy term: how much a value belongs to it, 0 to 1.
#[derive(Debug, Clone, PartialEq)]
pub enum Membership {
    /// `[a, b, c]`: rising from `a`, 1 at `b`, falling to `c`.
    Triangle([f64; 3]),
    /// `[a, b, c, d]`: rising from `a`, 1 from `b` to `c`, falling to `d`.
    Trapezoid([f64; 4]),
    /// A bell around `mean`, `sigma` wide.
    Gaussian { mean: f64, sigma: f64 },
}

impl Membership {
    /// How much `x` belongs to the term. An edge whose two points coincide is
    /// a shoulder: 1 on the far side of it, as simpful reads it.
    pub fn degree(&self, x: f64) -> f64 {
        let raw = match *self {
            Membership::Triangle([a, b, c]) => {
                if x < b {
                    rise(x, a, b)
                } else {
                    fall(x, b, c)
                }
            }
            Membership::Trapezoid([a, b, c, d]) => {
                if x < b {
                    rise(x, a, b)
                } else if x <= c {
                    1.0
                } else {
                    fall(x, c, d)
                }
            }
            Membership::Gaussian { mean, sigma } => {
                (-(x - mean).powi(2) / (2.0 * sigma * sigma)).exp()
            }
        };
        raw.clamp(0.0, 1.0)
    }
}

fn rise(x: f64, from: f64, to: f64) -> f64 {
    if from == to {
        1.0
    } else {
        (x - from) / (to - from)
    }
}

fn fall(x: f64, from: f64, to: f64) -> f64 {
    if from == to {
        1.0
    } else {
        1.0 - (x - from) / (to - from)
    }
}

/// An input of a rule base: its universe, its terms, and the value it reads
/// when none is given.
#[derive(Debug, Clone, PartialEq)]
pub struct Input {
    pub name: String,
    pub low: f64,
    pub high: f64,
    pub terms: IndexMap<String, Membership>,
    pub default: Option<f64>,
}

impl Input {
    fn clamp(&self, x: f64) -> f64 {
        x.clamp(self.low, self.high)
    }

    fn samples(&self, count: usize) -> Vec<f64> {
        let step = (self.high - self.low) / (count - 1) as f64;
        (0..count).map(|i| self.low + i as f64 * step).collect()
    }
}

/// A rule's antecedent: clauses `(input IS term)`, by input index.
#[derive(Debug, Clone, PartialEq)]
enum Expr {
    Is(usize, String),
    Not(Box<Expr>),
    And(Vec<Expr>),
    Or(Vec<Expr>),
}

impl Expr {
    fn holds(&self, inputs: &[Input], values: &[f64]) -> f64 {
        match self {
            Expr::Is(i, term) => inputs[*i].terms[term].degree(values[*i]),
            Expr::Not(e) => 1.0 - e.holds(inputs, values),
            Expr::And(es) => es
                .iter()
                .map(|e| e.holds(inputs, values))
                .fold(f64::INFINITY, f64::min),
            Expr::Or(es) => es
                .iter()
                .map(|e| e.holds(inputs, values))
                .fold(f64::NEG_INFINITY, f64::max),
        }
    }
}

/// One rule: when it fires, and the value of each output it gives.
#[derive(Debug, Clone, PartialEq)]
pub struct Rule {
    pub antecedent: String,
    expr: Expr,
    pub then: IndexMap<String, f64>,
}

/// A rule base: inputs, and rules giving each of its outputs.
#[derive(Debug, Clone, PartialEq)]
pub struct RuleBase {
    pub name: String,
    pub inputs: Vec<Input>,
    pub rules: Vec<Rule>,
    pub outputs: Vec<String>,
}

impl RuleBase {
    /// Each output at `values` (one per input, in order), each clamped to its
    /// input's universe.
    pub fn evaluate(&self, values: &[f64]) -> Result<IndexMap<String, f64>> {
        let clamped: Vec<f64> = self
            .inputs
            .iter()
            .zip(values)
            .map(|(i, v)| i.clamp(*v))
            .collect();
        let mut total = 0.0;
        let mut sums: IndexMap<String, f64> =
            self.outputs.iter().map(|o| (o.clone(), 0.0)).collect();
        for rule in &self.rules {
            let w = rule.expr.holds(&self.inputs, &clamped);
            total += w;
            for (output, value) in &rule.then {
                sums[output] += w * value;
            }
        }
        if !(total > 0.0) {
            let at: Vec<String> = self
                .inputs
                .iter()
                .zip(values)
                .map(|(i, v)| format!("{}={}", i.name, py_float_repr(*v)))
                .collect();
            return Err(Error::value(format!("no rule fires at {}", at.join(", "))));
        }
        Ok(sums.into_iter().map(|(o, s)| (o, s / total)).collect())
    }

    /// Some rule fires at every point of a grid over the inputs' universes.
    fn check(&self) -> Result<()> {
        let n = self.inputs.len() as i32;
        let per = (CHECK_TOTAL.powf(1.0 / n as f64) as usize).clamp(2, CHECK_POINTS);
        let grids: Vec<Vec<f64>> = self.inputs.iter().map(|i| i.samples(per)).collect();
        let mut index = vec![0usize; grids.len()];
        loop {
            let point: Vec<f64> = index.iter().zip(&grids).map(|(k, g)| g[*k]).collect();
            self.evaluate(&point)?;
            // The next point, the last input fastest.
            let mut d = grids.len();
            loop {
                if d == 0 {
                    return Ok(());
                }
                d -= 1;
                index[d] += 1;
                if index[d] < grids[d].len() {
                    break;
                }
                index[d] = 0;
            }
        }
    }
}

/// How the outputs of a quantity's rule bases are joined.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Combine {
    Min,
    Max,
}

/// A quantity: rule bases whose outputs `combine` joins.
#[derive(Debug, Clone, PartialEq)]
pub struct Quantity {
    pub name: String,
    /// Where it was read from, for messages.
    pub source: String,
    pub combine: Combine,
    pub rule_bases: Vec<RuleBase>,
    pub outputs: Vec<String>,
}

impl Quantity {
    /// The inputs its rule bases read, each once, in the order first read.
    pub fn inputs(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for base in &self.rule_bases {
            for i in &base.inputs {
                if !out.contains(&i.name) {
                    out.push(i.name.clone());
                }
            }
        }
        out
    }

    /// Each output at `inputs`, by name; an input not given reads its default.
    pub fn evaluate(&self, inputs: &IndexMap<String, f64>) -> Result<IndexMap<String, f64>> {
        let mut joined: Option<IndexMap<String, f64>> = None;
        for base in &self.rule_bases {
            let mut values = Vec::with_capacity(base.inputs.len());
            for i in &base.inputs {
                match inputs.get(&i.name).copied().or(i.default) {
                    Some(v) if v.is_finite() => values.push(v),
                    Some(v) => {
                        return Err(Error::value(format!(
                            "{}: {}.{}: {} is {}, not a finite number",
                            self.source,
                            self.name,
                            base.name,
                            i.name,
                            py_float_repr(v)
                        )))
                    }
                    None => {
                        return Err(Error::value(format!(
                            "{}: {}.{} reads {}, which was not given and has no default",
                            self.source, self.name, base.name, i.name
                        )))
                    }
                }
            }
            let out = base.evaluate(&values).map_err(|e| {
                Error::value(format!("{}: {}.{}: {e}", self.source, self.name, base.name))
            })?;
            joined = Some(match joined {
                None => out,
                Some(mut acc) => {
                    for (o, v) in out {
                        let a = acc[&o];
                        acc[&o] = match self.combine {
                            Combine::Min => a.min(v),
                            Combine::Max => a.max(v),
                        };
                    }
                    acc
                }
            });
        }
        Ok(joined.expect("a quantity has a rule base"))
    }
}

/// Quantities, read from a rule file.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RuleSet {
    pub quantities: IndexMap<String, Quantity>,
}

impl RuleSet {
    /// The rule set of a rule file's `text`; `source` names it in messages.
    pub fn parse(text: &str, source: &str) -> Result<Self> {
        let docs = parse_all(text).map_err(|e| Error::value(format!("{source}: {e}")))?;
        let [doc] = docs.as_slice() else {
            return Err(Error::value(format!(
                "{source}: expected one YAML document, got {}",
                docs.len()
            )));
        };
        let at = Where::new(source);
        let map = mapping(doc, &at, &["version", "quantities"])?;
        match get(map, "version").and_then(Node::as_i64) {
            Some(VERSION) => {}
            _ => {
                return Err(at.error(format!(
                    "version must be {VERSION}, got {}",
                    get(map, "version")
                        .map(NodeExt::py_repr)
                        .unwrap_or("None".into())
                )))
            }
        }
        let at = at.key("quantities");
        let quantities = required_mapping(map, "quantities", &at)?;
        let mut out = IndexMap::new();
        for (name, raw) in quantities {
            let name = node_name(name, &at)?;
            let quantity = read_quantity(&name, raw, source, &at.key(&name))?;
            out.insert(name, quantity);
        }
        Ok(RuleSet { quantities: out })
    }

    /// This rule set with `over`'s quantities in place of its own, quantity by
    /// quantity.
    pub fn merged(&self, over: &RuleSet) -> RuleSet {
        let mut quantities = self.quantities.clone();
        for (name, q) in &over.quantities {
            quantities.insert(name.clone(), q.clone());
        }
        RuleSet { quantities }
    }

    pub fn quantity(&self, name: &str) -> Result<&Quantity> {
        self.quantities.get(name).ok_or_else(|| {
            let mut known: Vec<String> = self.quantities.keys().cloned().collect();
            known.sort();
            Error::value(format!(
                "unknown quantity '{name}'; the rules give {}",
                crate::model::py_list(&known)
            ))
        })
    }

    /// Each output of `quantity` at `inputs` ([`Quantity::evaluate`]).
    pub fn evaluate(
        &self,
        quantity: &str,
        inputs: &IndexMap<String, f64>,
    ) -> Result<IndexMap<String, f64>> {
        self.quantity(quantity)?.evaluate(inputs)
    }

    /// Refuse an input that is not one of `allowed` (the backend's).
    pub fn check_inputs(&self, allowed: &[String]) -> Result<()> {
        for q in self.quantities.values() {
            for base in &q.rule_bases {
                for i in &base.inputs {
                    if !allowed.contains(&i.name) {
                        return Err(Error::value(format!(
                            "{}: quantities.{}.rule_bases.{}.inputs: unknown input '{}'; the rules may read {}",
                            q.source,
                            q.name,
                            base.name,
                            i.name,
                            crate::model::py_list(allowed)
                        )));
                    }
                }
            }
        }
        Ok(())
    }
}

/// Where in a file a node is, for messages: `file: quantities.x.rules[0]`.
#[derive(Clone)]
struct Where {
    source: String,
    path: String,
}

impl Where {
    fn new(source: &str) -> Self {
        Where {
            source: source.to_string(),
            path: String::new(),
        }
    }

    fn key(&self, key: &str) -> Self {
        let path = if self.path.is_empty() {
            key.to_string()
        } else {
            format!("{}.{key}", self.path)
        };
        Where {
            source: self.source.clone(),
            path,
        }
    }

    fn index(&self, i: usize) -> Self {
        Where {
            source: self.source.clone(),
            path: format!("{}[{i}]", self.path),
        }
    }

    fn error(&self, message: impl std::fmt::Display) -> Error {
        if self.path.is_empty() {
            Error::value(format!("{}: {message}", self.source))
        } else {
            Error::value(format!("{}: {}: {message}", self.source, self.path))
        }
    }
}

fn untag(node: &Node) -> &Node {
    match node {
        Node::Tagged(t) => untag(&t.value),
        other => other,
    }
}

fn get<'a>(map: &'a crate::yaml::Mapping, key: &str) -> Option<&'a Node> {
    map.get(key).map(untag)
}

/// `node` as a mapping with only the keys in `known`.
fn mapping<'a>(node: &'a Node, at: &Where, known: &[&str]) -> Result<&'a crate::yaml::Mapping> {
    let Node::Mapping(map) = untag(node) else {
        return Err(at.error(format!("expected a mapping, got {}", node.py_repr())));
    };
    if !known.is_empty() {
        let mut unknown: Vec<String> = map
            .keys()
            .map(NodeExt::py_str)
            .filter(|k| !known.contains(&k.as_str()))
            .collect();
        if !unknown.is_empty() {
            unknown.sort();
            return Err(at.error(format!(
                "unknown fields {}",
                crate::model::py_list(&unknown)
            )));
        }
    }
    Ok(map)
}

fn required_mapping<'a>(
    map: &'a crate::yaml::Mapping,
    key: &str,
    at: &Where,
) -> Result<&'a crate::yaml::Mapping> {
    match get(map, key) {
        Some(node) => mapping(node, at, &[]),
        None => Err(at.error("missing")),
    }
}

fn number(node: &Node, at: &Where) -> Result<f64> {
    let node = untag(node);
    match node {
        Node::Number(n) => match n.as_f64() {
            Some(x) if x.is_finite() => Ok(x),
            _ => Err(at.error(format!("expected a finite number, got {}", node.py_repr()))),
        },
        _ => Err(at.error(format!("expected a number, got {}", node.py_repr()))),
    }
}

fn numbers(node: &Node, at: &Where) -> Result<Vec<f64>> {
    let Node::Sequence(items) = untag(node) else {
        return Err(at.error(format!("expected a list, got {}", node.py_repr())));
    };
    items.iter().map(|n| number(n, at)).collect()
}

fn is_name(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !matches!(s, "AND" | "OR" | "NOT" | "IS")
}

fn node_name(node: &Node, at: &Where) -> Result<String> {
    let s = node.py_str();
    if matches!(untag(node), Node::String(_)) && is_name(&s) {
        Ok(s)
    } else {
        Err(at.error(format!(
            "{} is not a name (letters, digits, _)",
            node.py_repr()
        )))
    }
}

fn read_term(node: &Node, at: &Where) -> Result<Membership> {
    let map = mapping(node, at, &[])?;
    if map.len() != 1 {
        return Err(at.error("expected one of triangle, trapezoid, gaussian"));
    }
    let (shape, raw) = map.iter().next().expect("one entry");
    let shape = shape.py_str();
    let at = at.key(&shape);
    let size = match shape.as_str() {
        "triangle" => 3,
        "trapezoid" => 4,
        "gaussian" => 2,
        _ => {
            return Err(at.error(format!(
                "unknown term shape '{shape}' (triangle, trapezoid, gaussian)"
            )))
        }
    };
    let p = numbers(raw, &at)?;
    if p.len() != size {
        return Err(at.error(format!("expected {size} numbers, got {}", p.len())));
    }
    if shape == "gaussian" {
        if !(p[1] > 0.0) {
            return Err(at.error("sigma must be positive"));
        }
        return Ok(Membership::Gaussian {
            mean: p[0],
            sigma: p[1],
        });
    }
    if p.windows(2).any(|w| w[0] > w[1]) {
        return Err(at.error("the points must not decrease"));
    }
    Ok(if size == 3 {
        Membership::Triangle([p[0], p[1], p[2]])
    } else {
        Membership::Trapezoid([p[0], p[1], p[2], p[3]])
    })
}

fn read_input(name: &str, node: &Node, at: &Where) -> Result<Input> {
    let map = mapping(node, at, &["universe", "terms", "default"])?;
    let universe_at = at.key("universe");
    let universe = match get(map, "universe") {
        Some(n) => numbers(n, &universe_at)?,
        None => return Err(universe_at.error("missing")),
    };
    let [low, high] = universe[..] else {
        return Err(universe_at.error("expected [low, high]"));
    };
    if !(low < high) {
        return Err(universe_at.error("low must be below high"));
    }
    let terms_at = at.key("terms");
    let mut terms = IndexMap::new();
    for (term, spec) in required_mapping(map, "terms", &terms_at)? {
        let term = node_name(term, &terms_at)?;
        let membership = read_term(spec, &terms_at.key(&term))?;
        terms.insert(term, membership);
    }
    if terms.is_empty() {
        return Err(terms_at.error("no terms"));
    }
    let default = match get(map, "default") {
        None | Some(Node::Null) => None,
        Some(n) => Some(number(n, &at.key("default"))?),
    };
    let input = Input {
        name: name.to_string(),
        low,
        high,
        terms,
        default,
    };
    for x in input.samples(CHECK_POINTS) {
        let most = input
            .terms
            .values()
            .map(|m| m.degree(x))
            .fold(0.0, f64::max);
        if !(most > 0.0) {
            return Err(terms_at.error(format!(
                "no term covers {name} = {}, within its universe",
                py_float_repr(x)
            )));
        }
    }
    Ok(input)
}

fn read_quantity(name: &str, node: &Node, source: &str, at: &Where) -> Result<Quantity> {
    let map = mapping(node, at, &["combine", "rule_bases"])?;
    let combine = match get(map, "combine").map(NodeExt::py_str).as_deref() {
        None | Some("min") => Combine::Min,
        Some("max") => Combine::Max,
        Some(other) => {
            return Err(at
                .key("combine")
                .error(format!("expected min or max, got '{other}'")))
        }
    };
    let bases_at = at.key("rule_bases");
    let mut rule_bases = Vec::new();
    for (base, raw) in required_mapping(map, "rule_bases", &bases_at)? {
        let base = node_name(base, &bases_at)?;
        rule_bases.push(read_rule_base(&base, raw, &bases_at.key(&base))?);
    }
    let Some(first) = rule_bases.first() else {
        return Err(bases_at.error("no rule bases"));
    };
    let outputs = first.outputs.clone();
    for base in &rule_bases {
        let mut theirs = base.outputs.clone();
        let mut ours = outputs.clone();
        theirs.sort();
        ours.sort();
        if theirs != ours {
            return Err(bases_at.error(format!(
                "every rule base gives the same outputs: {} gives {}, {} gives {}",
                first.name,
                crate::model::py_list(&ours),
                base.name,
                crate::model::py_list(&theirs)
            )));
        }
    }
    Ok(Quantity {
        name: name.to_string(),
        source: source.to_string(),
        combine,
        rule_bases,
        outputs,
    })
}

fn read_rule_base(name: &str, node: &Node, at: &Where) -> Result<RuleBase> {
    let map = mapping(node, at, &["inputs", "rules"])?;
    let inputs_at = at.key("inputs");
    let mut inputs = Vec::new();
    for (input, raw) in required_mapping(map, "inputs", &inputs_at)? {
        let input = node_name(input, &inputs_at)?;
        inputs.push(read_input(&input, raw, &inputs_at.key(&input))?);
    }
    if inputs.is_empty() {
        return Err(inputs_at.error("no inputs"));
    }
    let rules_at = at.key("rules");
    let items = match get(map, "rules") {
        Some(Node::Sequence(items)) if !items.is_empty() => items,
        _ => return Err(rules_at.error("expected a list of rules")),
    };
    let mut rules = Vec::new();
    for (k, raw) in items.iter().enumerate() {
        rules.push(read_rule(raw, &inputs, &rules_at.index(k))?);
    }
    let outputs: Vec<String> = rules[0].then.keys().cloned().collect();
    for (k, rule) in rules.iter().enumerate() {
        let mut theirs: Vec<&String> = rule.then.keys().collect();
        let mut ours: Vec<&String> = outputs.iter().collect();
        theirs.sort();
        ours.sort();
        if theirs != ours {
            return Err(rules_at.index(k).key("then").error(format!(
                "every rule gives the same outputs: {}",
                crate::model::py_list(&outputs)
            )));
        }
    }
    let base = RuleBase {
        name: name.to_string(),
        inputs,
        rules,
        outputs,
    };
    base.check().map_err(|e| at.error(e))?;
    Ok(base)
}

fn read_rule(node: &Node, inputs: &[Input], at: &Where) -> Result<Rule> {
    let map = mapping(node, at, &["if", "then"])?;
    let if_at = at.key("if");
    let antecedent = match get(map, "if") {
        Some(Node::String(s)) if !s.trim().is_empty() => s.clone(),
        _ => return Err(if_at.error("expected an antecedent")),
    };
    let expr = Parser::new(&antecedent, inputs)
        .parse()
        .map_err(|m| if_at.error(m))?;
    let then_at = at.key("then");
    let mut then = IndexMap::new();
    for (output, value) in required_mapping(map, "then", &then_at)? {
        let output = node_name(output, &then_at)?;
        let v = number(value, &then_at.key(&output))?;
        then.insert(output, v);
    }
    if then.is_empty() {
        return Err(then_at.error("no outputs"));
    }
    if let (Some(low), Some(high)) = (then.get("low"), then.get("high")) {
        if low > high {
            return Err(then_at.error("low must not be above high"));
        }
    }
    Ok(Rule {
        antecedent,
        expr,
        then,
    })
}

/// Reads an antecedent: clauses `(input IS term)`, `NOT`, and `AND` or `OR`
/// (not both in one group: parenthesize).
struct Parser<'a> {
    tokens: Vec<String>,
    at: usize,
    inputs: &'a [Input],
}

impl<'a> Parser<'a> {
    fn new(text: &str, inputs: &'a [Input]) -> Self {
        let mut tokens = Vec::new();
        let mut word = String::new();
        for c in text.chars() {
            if c == '(' || c == ')' || c.is_whitespace() {
                if !word.is_empty() {
                    tokens.push(std::mem::take(&mut word));
                }
                if !c.is_whitespace() {
                    tokens.push(c.to_string());
                }
            } else {
                word.push(c);
            }
        }
        if !word.is_empty() {
            tokens.push(word);
        }
        Parser {
            tokens,
            at: 0,
            inputs,
        }
    }

    fn peek(&self, ahead: usize) -> Option<&str> {
        self.tokens.get(self.at + ahead).map(String::as_str)
    }

    fn next(&mut self) -> Option<String> {
        let t = self.tokens.get(self.at).cloned();
        self.at += 1;
        t
    }

    fn parse(mut self) -> std::result::Result<Expr, String> {
        let expr = self.expr()?;
        match self.peek(0) {
            None => Ok(expr),
            Some(t) => Err(format!("unexpected '{t}'")),
        }
    }

    fn expr(&mut self) -> std::result::Result<Expr, String> {
        let first = self.unary()?;
        let mut op: Option<String> = None;
        let mut items = vec![first];
        while let Some(t) = self.peek(0) {
            if t != "AND" && t != "OR" {
                break;
            }
            let t = t.to_string();
            match &op {
                Some(o) if *o != t => {
                    return Err("AND and OR in one group: parenthesize one".into());
                }
                _ => op = Some(t),
            }
            self.at += 1;
            items.push(self.unary()?);
        }
        Ok(match op.as_deref() {
            None => items.pop().expect("one item"),
            Some("AND") => Expr::And(items),
            _ => Expr::Or(items),
        })
    }

    fn unary(&mut self) -> std::result::Result<Expr, String> {
        match self.peek(0) {
            Some("NOT") => {
                self.at += 1;
                Ok(Expr::Not(Box::new(self.unary()?)))
            }
            Some("(") => {
                self.at += 1;
                let inner = if self.peek(1) == Some("IS") {
                    self.clause()?
                } else {
                    self.expr()?
                };
                match self.next().as_deref() {
                    Some(")") => Ok(inner),
                    _ => Err("expected ')'".into()),
                }
            }
            Some(t) => Err(format!(
                "expected clauses such as (water IS wet), got '{t}'"
            )),
            None => Err("the antecedent ends too soon".into()),
        }
    }

    fn clause(&mut self) -> std::result::Result<Expr, String> {
        let variable = self.next().expect("peeked");
        self.at += 1; // IS
        let Some(term) = self.next() else {
            return Err(format!("({variable} IS ...) names no term"));
        };
        let Some(index) = self.inputs.iter().position(|i| i.name == variable) else {
            let known: Vec<String> = self.inputs.iter().map(|i| i.name.clone()).collect();
            return Err(format!(
                "'{variable}' is no input of this rule base ({})",
                crate::model::py_list(&known)
            ));
        };
        if !self.inputs[index].terms.contains_key(&term) {
            let known: Vec<String> = self.inputs[index].terms.keys().cloned().collect();
            return Err(format!(
                "{variable} has no term '{term}' ({})",
                crate::model::py_list(&known)
            ));
        }
        Ok(Expr::Is(index, term))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GRIP: &str = r#"
version: 1
quantities:
  grip:
    rule_bases:
      water:
        inputs:
          water:
            universe: [0, 100]
            terms:
              dry: {trapezoid: [0, 0, 0, 50]}
              wet: {triangle: [0, 50, 100]}
              flooded: {trapezoid: [50, 100, 100, 100]}
        rules:
          - if: (water IS dry)
            then: {low: 0.90, high: 1.10}
          - if: (water IS wet)
            then: {low: 0.65, high: 0.85}
          - if: (water IS flooded)
            then: {low: 0.45, high: 0.65}
"#;

    fn at(rules: &RuleSet, q: &str, inputs: &[(&str, f64)]) -> Result<(f64, f64)> {
        let inputs = inputs.iter().map(|(k, v)| (k.to_string(), *v)).collect();
        let out = rules.evaluate(q, &inputs)?;
        Ok((out["low"], out["high"]))
    }

    fn close(a: (f64, f64), b: (f64, f64)) -> bool {
        (a.0 - b.0).abs() < 1e-12 && (a.1 - b.1).abs() < 1e-12
    }

    #[test]
    fn one_term_rules_read_linearly_between_the_peaks() {
        let rules = RuleSet::parse(GRIP, "grip.yaml").unwrap();
        assert!(close(
            at(&rules, "grip", &[("water", 0.0)]).unwrap(),
            (0.9, 1.1)
        ));
        assert!(close(
            at(&rules, "grip", &[("water", 25.0)]).unwrap(),
            (0.775, 0.975)
        ));
        assert!(close(
            at(&rules, "grip", &[("water", 100.0)]).unwrap(),
            (0.45, 0.65)
        ));
        // Clamped to the universe.
        assert!(close(
            at(&rules, "grip", &[("water", 150.0)]).unwrap(),
            (0.45, 0.65)
        ));
        assert!(close(
            at(&rules, "grip", &[("water", -5.0)]).unwrap(),
            (0.9, 1.1)
        ));
    }

    #[test]
    fn and_or_not_are_min_max_and_complement() {
        let text = r#"
version: 1
quantities:
  q:
    rule_bases:
      b:
        inputs:
          x:
            universe: [0, 1]
            terms: {lo: {trapezoid: [0, 0, 0, 1]}, hi: {trapezoid: [0, 1, 1, 1]}}
          y:
            universe: [0, 1]
            terms: {lo: {trapezoid: [0, 0, 0, 1]}, hi: {trapezoid: [0, 1, 1, 1]}}
        rules:
          - if: (x IS hi) AND (y IS hi)
            then: {v: 1}
          - if: ((x IS lo) OR (y IS lo)) AND (NOT (x IS hi))
            then: {v: 0}
          - if: (x IS hi) AND (y IS lo)
            then: {v: 0.5}
"#;
        let rules = RuleSet::parse(text, "q.yaml").unwrap();
        let inputs: IndexMap<String, f64> = [("x".to_string(), 0.25), ("y".to_string(), 0.75)]
            .into_iter()
            .collect();
        // The rules fire min(0.25, 0.75) = 0.25, min(max(0.75, 0.25), 1 - 0.25)
        // = 0.75 and min(0.25, 0.25) = 0.25.
        let v = rules.evaluate("q", &inputs).unwrap()["v"];
        assert!((v - (0.25 * 1.0 + 0.25 * 0.5) / (0.25 + 0.75 + 0.25)).abs() < 1e-12);
    }

    #[test]
    fn combine_joins_the_rule_bases_output_by_output() {
        let base = |name: &str, low: f64| {
            format!(
                "      {name}:\n        inputs:\n          x:\n            universe: [0, 1]\n            terms: {{any: {{trapezoid: [0, 0, 1, 1]}}}}\n        rules:\n          - if: (x IS any)\n            then: {{low: {low}, high: 400}}\n"
            )
        };
        for (combine, expected) in [("min", 100.0), ("max", 200.0)] {
            let text = format!(
                "version: 1\nquantities:\n  v:\n    combine: {combine}\n    rule_bases:\n{}{}",
                base("a", 100.0),
                base("b", 200.0)
            );
            let rules = RuleSet::parse(&text, "v.yaml").unwrap();
            assert!(close(
                at(&rules, "v", &[("x", 0.5)]).unwrap(),
                (expected, 400.0)
            ));
        }
    }

    #[test]
    fn a_later_set_replaces_quantities_by_name() {
        let base = RuleSet::parse(GRIP, "base.yaml").unwrap();
        let over = RuleSet::parse(&GRIP.replace("0.45", "0.30"), "over.yaml").unwrap();
        let merged = base.merged(&over);
        assert!(close(
            at(&merged, "grip", &[("water", 100.0)]).unwrap(),
            (0.30, 0.65)
        ));
        assert_eq!(merged.quantity("grip").unwrap().source, "over.yaml");
    }

    #[test]
    fn a_missing_input_reads_its_default_or_is_refused() {
        let rules = RuleSet::parse(GRIP, "grip.yaml").unwrap();
        let e = at(&rules, "grip", &[]).unwrap_err().to_string();
        assert!(e.contains("reads water, which was not given"), "{e}");
        let with_default = GRIP.replace(
            "universe: [0, 100]",
            "universe: [0, 100]\n            default: 100",
        );
        let rules = RuleSet::parse(&with_default, "grip.yaml").unwrap();
        assert!(close(at(&rules, "grip", &[]).unwrap(), (0.45, 0.65)));
    }

    #[test]
    fn malformed_rule_files_are_refused_saying_where() {
        let cases = [
            (GRIP.replace("(water IS wet)", "(water IS soaked)"), "water has no term 'soaked'"),
            (GRIP.replace("(water IS wet)", "(fog IS wet)"), "'fog' is no input"),
            (GRIP.replace("(water IS wet)", "water IS wet"), "expected clauses such as"),
            (
                GRIP.replace("(water IS wet)", "(water IS wet) AND (water IS dry) OR (water IS dry)"),
                "parenthesize",
            ),
            (GRIP.replace("low: 0.65", "low: 0.95"), "low must not be above high"),
            (GRIP.replace("{low: 0.65, high: 0.85}", "{low: 0.65}"), "same outputs"),
            (GRIP.replace("triangle: [0, 50, 100]", "triangle: [50, 0, 100]"), "must not decrease"),
            (GRIP.replace("triangle: [0, 50, 100]", "bell: [0, 1]"), "unknown term shape 'bell'"),
            (GRIP.replace("universe: [0, 100]", "universe: [100, 0]"), "low must be below high"),
            (GRIP.replace("version: 1", "version: 2"), "version must be 1"),
            (GRIP.replace("rule_bases:", "combine: mean\n    rule_bases:"), "expected min or max"),
            // Nothing covers 50 exactly.
            (
                GRIP.replace("dry: {trapezoid: [0, 0, 0, 50]}", "dry: {trapezoid: [0, 0, 0, 40]}")
                    .replace("wet: {triangle: [0, 50, 100]}", "wet: {triangle: [60, 70, 100]}"),
                "no term covers water",
            ),
            // No rule for flooded: no rule fires on a flooded road.
            (
                GRIP.replace(
                    "          - if: (water IS flooded)\n            then: {low: 0.45, high: 0.65}\n",
                    "",
                )
                .replace("wet: {triangle: [0, 50, 100]}", "wet: {triangle: [0, 50, 90]}"),
                "no rule fires at water=",
            ),
        ];
        for (text, expected) in cases {
            let e = RuleSet::parse(&text, "grip.yaml").unwrap_err().to_string();
            assert!(e.contains(expected), "expected {expected:?} in {e:?}");
            assert!(e.starts_with("grip.yaml: "), "{e}");
        }
    }

    #[test]
    fn the_backend_says_which_inputs_exist() {
        let rules = RuleSet::parse(GRIP, "grip.yaml").unwrap();
        assert!(rules.check_inputs(&["water".into()]).is_ok());
        let e = rules
            .check_inputs(&["rain".into()])
            .unwrap_err()
            .to_string();
        assert!(e.contains("unknown input 'water'"), "{e}");
    }

    #[test]
    fn a_shoulder_is_one_beyond_its_edge() {
        assert_eq!(
            Membership::Trapezoid([0.0, 0.0, 0.0, 50.0]).degree(-10.0),
            1.0
        );
        assert_eq!(Membership::Triangle([0.0, 50.0, 100.0]).degree(120.0), 0.0);
        assert_eq!(Membership::Triangle([0.0, 50.0, 100.0]).degree(25.0), 0.5);
        let g = Membership::Gaussian {
            mean: 0.0,
            sigma: 1.0,
        };
        assert!((g.degree(1.0) - (-0.5f64).exp()).abs() < 1e-15);
    }
}
