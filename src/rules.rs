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
//! input's universe covered by its terms, and some rule firing everywhere the
//! inputs can be -- read at the terms' corners and between them, which finds
//! any gap, since the terms are piecewise linear between their corners. A value outside an input's universe is
//! clamped to it. Which inputs exist is the backend's to say
//! ([`RuleSet::check_inputs`]); one an input names a `default` for may be left
//! out.

use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};
use crate::sources::GitSource;
use crate::value::py_float_repr;
use crate::yaml::{parse_all, Node, NodeExt};

/// The rule file format this module reads.
pub const VERSION: i64 = 1;
/// Where `exp(-x)` is zero in f64: a bell's degree past `sigma * sqrt(2 * this)`.
const EXP_UNDERFLOW: f64 = 745.2;
/// The deepest a chain of `extends` may go.
pub const MAX_EXTENDS_DEPTH: usize = 32;
/// The evenly spaced points per input the checks add to its terms' own.
const CHECK_SPACED: usize = 11;
/// The most points a rule base's firing check evaluates: one reading more
/// inputs than that lets be checked is refused.
pub const CHECK_LIMIT: usize = 1_000_000;

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
    /// `x` clamped to the universe.
    pub fn clamp(&self, x: f64) -> f64 {
        x.clamp(self.low, self.high)
    }

    /// The points the checks read the input at: its universe's ends, its
    /// terms' corners (and a bell's centre) inside it, a few evenly spaced
    /// ones, and the midpoint of each two neighbours. Its terms are piecewise
    /// linear between their corners, so where none covers it, or where no
    /// rule fires, begins and ends at a corner: one of these points is in it.
    fn check_points(&self) -> Vec<f64> {
        let mut corners: Vec<f64> = vec![self.low, self.high];
        for m in self.terms.values() {
            match m {
                Membership::Triangle(p) => corners.extend(p),
                Membership::Trapezoid(p) => corners.extend(p),
                Membership::Gaussian { mean, sigma } => {
                    // A bell is never zero, but its degree is, in floating
                    // point, past where exp() underflows: those are its corners.
                    let reach = sigma * (2.0 * EXP_UNDERFLOW).sqrt();
                    corners.extend([mean - reach, *mean, mean + reach]);
                }
            }
        }
        let step = (self.high - self.low) / (CHECK_SPACED - 1) as f64;
        corners.extend((0..CHECK_SPACED).map(|i| self.low + i as f64 * step));
        corners.retain(|x| self.low <= *x && *x <= self.high);
        corners.sort_by(f64::total_cmp);
        corners.dedup();
        let mut out = Vec::with_capacity(corners.len() * 2);
        for w in corners.windows(2) {
            out.push(w[0]);
            out.push((w[0] + w[1]) / 2.0);
        }
        out.push(*corners.last().expect("the universe's ends"));
        out
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

    /// How strongly each rule fires at `values` (one per input, in order),
    /// each clamped to its input's universe.
    pub fn firing(&self, values: &[f64]) -> Vec<f64> {
        let clamped: Vec<f64> = self
            .inputs
            .iter()
            .zip(values)
            .map(|(i, v)| i.clamp(*v))
            .collect();
        self.rules
            .iter()
            .map(|r| r.expr.holds(&self.inputs, &clamped))
            .collect()
    }

    /// Some rule fires everywhere the inputs can be: at every combination of
    /// their check points ([`Input::check_points`]). A clause reads one input,
    /// so where no rule fires is made of cells between those points.
    fn check(&self) -> Result<()> {
        let grids: Vec<Vec<f64>> = self.inputs.iter().map(Input::check_points).collect();
        let total = grids
            .iter()
            .try_fold(1usize, |acc, g| acc.checked_mul(g.len()))
            .filter(|t| *t <= CHECK_LIMIT);
        if total.is_none() {
            return Err(Error::value(format!(
                "its {} inputs and their terms make more than {CHECK_LIMIT} points to check; \
                 split it into rule bases reading fewer inputs",
                self.inputs.len()
            )));
        }
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
        for (_, out) in self.evaluate_each(inputs)? {
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

    /// Each rule base's outputs at `inputs`, before `combine` joins them.
    pub fn evaluate_each(
        &self,
        inputs: &IndexMap<String, f64>,
    ) -> Result<Vec<(String, IndexMap<String, f64>)>> {
        let mut out = Vec::with_capacity(self.rule_bases.len());
        for base in &self.rule_bases {
            let values = self.values_of(base, inputs)?;
            let outputs = base.evaluate(&values).map_err(|e| {
                Error::value(format!("{}: {}.{}: {e}", self.source, self.name, base.name))
            })?;
            out.push((base.name.clone(), outputs));
        }
        Ok(out)
    }

    /// The values `base` reads of `inputs`, in its inputs' order: each given
    /// one, else its default.
    pub fn values_of(&self, base: &RuleBase, inputs: &IndexMap<String, f64>) -> Result<Vec<f64>> {
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
        Ok(values)
    }
}

/// A rule file another one `extends`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Reference {
    /// A path: relative to the file that names it (a local file, a URL, or a
    /// file in a git checkout), or absolute.
    Path(String),
    /// An `https://` URL, and the SHA-256 its content must have, if pinned.
    Url { url: String, sha256: Option<String> },
    /// A file in a git repository at a revision ([`crate::sources`]).
    Git(GitSource),
}

/// Where a rule file was read from: what a relative [`Reference::Path`] in
/// it is relative to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// Given as text: a relative path is relative to the working directory.
    Text,
    File(PathBuf),
    Url(String),
    Git(GitSource),
}

impl Origin {
    /// The origin a reference made in a file from here names.
    pub fn of(&self, reference: &Reference) -> Result<Origin> {
        Ok(match reference {
            Reference::Url { url, .. } => Origin::Url(url.clone()),
            Reference::Git(g) => Origin::Git(g.clone()),
            Reference::Path(p) => match self {
                Origin::Url(base) if !is_absolute_path(p) => Origin::Url(url_join(base, p)?),
                Origin::Git(g) if !is_absolute_path(p) => Origin::Git(GitSource::new(
                    g.url.clone(),
                    g.rev.clone(),
                    git_join(&g.path, p),
                )?),
                Origin::File(f) if !is_absolute_path(p) => {
                    Origin::File(normalize(&f.parent().unwrap_or(Path::new("")).join(p)))
                }
                _ => Origin::File(normalize(Path::new(p))),
            },
        })
    }

    /// The same file's key, however it is named: a local file's canonical
    /// path (links followed), when it exists.
    fn key(&self) -> String {
        match self {
            Origin::File(p) => p
                .canonicalize()
                .map(|c| c.display().to_string())
                .unwrap_or_else(|_| self.label()),
            _ => self.label(),
        }
    }

    /// How it is named in messages.
    pub fn label(&self) -> String {
        match self {
            Origin::Text => "<text>".into(),
            Origin::File(p) => p.display().to_string(),
            Origin::Url(u) => u.clone(),
            Origin::Git(g) => format!("{}@{}:{}", g.url, g.rev, g.path),
        }
    }
}

/// `path` with `.` and `..` worked out without the file system (a leading
/// `..` of a relative path is kept).
fn normalize(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out: Vec<Component> = Vec::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => match out.last() {
                Some(Component::Normal(_)) => {
                    out.pop();
                }
                Some(Component::RootDir) | Some(Component::Prefix(_)) => {}
                _ => out.push(part),
            },
            other => out.push(other),
        }
    }
    out.iter().collect()
}

fn is_absolute_path(p: &str) -> bool {
    Path::new(p).is_absolute() || p.starts_with("https://") || p.starts_with("http://")
}

/// `rel` against the URL `base`, its `.` and `..` worked out.
fn url_join(base: &str, rel: &str) -> Result<String> {
    let Some(rest) = base.strip_prefix("https://") else {
        return Err(Error::value(format!("{base}: only https:// URLs are read")));
    };
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    let mut parts: Vec<&str> = path.split('/').collect();
    parts.pop(); // the file's own name
    for part in rel.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    Ok(format!("https://{host}/{}", parts.join("/")))
}

/// `rel` against the directory of `path`, inside a repository.
fn git_join(path: &str, rel: &str) -> String {
    let mut parts: Vec<&str> = path.split('/').collect();
    parts.pop();
    for part in rel.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    parts.join("/")
}

/// Reads the rule files a rule file `extends`.
pub trait Fetch {
    /// The text of the file at `origin` (never [`Origin::Text`]).
    fn fetch(&mut self, origin: &Origin) -> Result<String>;
}

/// Reads local files and git sources itself, and asks `url` for an
/// `https://` URL (the host's: Python's urllib, the browser's fetch); without
/// one, a URL is an error.
pub struct Files<'a> {
    pub url: Option<&'a mut FetchUrl<'a>>,
}

/// Reads an `https://` URL's text: the host's.
pub type FetchUrl<'a> = dyn FnMut(&str) -> Result<String> + 'a;

impl Fetch for Files<'_> {
    fn fetch(&mut self, origin: &Origin) -> Result<String> {
        match origin {
            Origin::Text => Err(Error::value("a rule file given as text has no file")),
            Origin::File(p) => std::fs::read_to_string(p)
                .map_err(|e| Error::value(format!("cannot read rules {}: {e}", p.display()))),
            Origin::Git(g) => {
                let (path, _) = crate::sources::checkout(g)?;
                std::fs::read_to_string(&path)
                    .map_err(|e| Error::value(format!("cannot read rules {}: {e}", origin.label())))
            }
            Origin::Url(u) => match self.url.as_mut() {
                Some(f) => f(u),
                None => Err(Error::value(format!(
                    "cannot read rules {u}: no way to fetch a URL was given"
                ))),
            },
        }
    }
}

/// Files already read, by [`Origin::label`]: what the browser fetched ahead.
pub struct Given(pub IndexMap<String, String>);

impl Fetch for Given {
    fn fetch(&mut self, origin: &Origin) -> Result<String> {
        self.0
            .get(&origin.label())
            .cloned()
            .ok_or_else(|| Error::value(format!("cannot read rules {}: not given", origin.label())))
    }
}

/// A rule file read, and the SHA-256 of its text: what a run records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRead {
    pub label: String,
    pub sha256: String,
}

/// Quantities, read from a rule file and the files it extends.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RuleSet {
    pub quantities: IndexMap<String, Quantity>,
    /// Every file read, the files extended before those extending them.
    pub files: Vec<FileRead>,
    /// The rules as one document, every `extends` worked in: what a later
    /// set merges over ([`RuleSet::merged`]).
    doc: Node,
}

impl RuleSet {
    /// The rule set of a rule file's `text`; `source` names it in messages.
    /// It may not extend another file ([`RuleSet::parse_with`] reads those).
    pub fn parse(text: &str, source: &str) -> Result<Self> {
        Self::parse_with(text, source, &Origin::Text, &mut Given(IndexMap::new()))
    }

    /// The rule set of the file at `origin` and the files it extends: a
    /// whole set, checked on its own ([`RuleSet::load_over`] reads one that
    /// only changes another).
    pub fn load(origin: &Origin, fetch: &mut dyn Fetch) -> Result<Self> {
        let text = fetch.fetch(origin)?;
        Self::parse_with(&text, &origin.label(), origin, fetch)
    }

    /// The rule set of a rule file's `text`, read from `origin` (what a
    /// relative path it extends is relative to), and the files it extends,
    /// read with `fetch`.
    pub fn parse_with(
        text: &str,
        source: &str,
        origin: &Origin,
        fetch: &mut dyn Fetch,
    ) -> Result<Self> {
        let mut files = Vec::new();
        let doc = compose(text, source, origin, fetch, &mut Vec::new(), &mut files)?;
        let mut set = Self::from_doc(doc, source)?;
        set.files = files;
        Ok(set)
    }

    /// The file at `origin`, and the files it extends, merged over these
    /// rules ([`RuleSet::merged`]): it need only name what it changes, since
    /// it is checked once merged.
    pub fn load_over(&self, origin: &Origin, fetch: &mut dyn Fetch) -> Result<Self> {
        let text = fetch.fetch(origin)?;
        self.parse_over(&text, &origin.label(), origin, fetch)
    }

    /// A rule file's `text` (read from `origin`), and the files it extends,
    /// merged over these rules: [`RuleSet::load_over`] for text in hand.
    pub fn parse_over(
        &self,
        text: &str,
        source: &str,
        origin: &Origin,
        fetch: &mut dyn Fetch,
    ) -> Result<Self> {
        let mut files = Vec::new();
        let over = compose(text, source, origin, fetch, &mut Vec::new(), &mut files)?;
        let touched: Vec<String> = over
            .get("quantities")
            .and_then(Node::as_mapping)
            .map(|m| m.keys().map(NodeExt::py_str).collect())
            .unwrap_or_default();
        let mut doc = self.doc.clone();
        merge_doc(&mut doc, &over);
        let mut set = Self::from_doc(doc, source)?;
        for (name, q) in &mut set.quantities {
            if !touched.contains(name) {
                if let Some(mine) = self.quantities.get(name) {
                    q.source = mine.source.clone();
                }
            }
        }
        set.files = self.files.iter().cloned().chain(files).collect();
        Ok(set)
    }

    /// The references a rule file's `text` extends, each as the origin it
    /// names from `origin`: what a host fetches ahead ([`Given`]).
    pub fn references(text: &str, source: &str, origin: &Origin) -> Result<Vec<Origin>> {
        let doc = document(text, source)?;
        extends_of(&doc, &Where::new(source))?
            .iter()
            .map(|r| origin.of(r))
            .collect()
    }

    fn from_doc(doc: Node, source: &str) -> Result<Self> {
        let at = Where::new(source);
        let map = mapping(&doc, &at, &["version", "quantities"])?;
        let at = at.key("quantities");
        let quantities = required_mapping(map, "quantities", &at)?;
        let mut out = IndexMap::new();
        for (name, raw) in quantities {
            let name = node_name(name, &at)?;
            let quantity = read_quantity(&name, raw, source, &at.key(&name))?;
            out.insert(name, quantity);
        }
        Ok(RuleSet {
            quantities: out,
            files: Vec::new(),
            doc,
        })
    }

    /// This rule set with `over`'s rules merged over it: a quantity, a rule
    /// base and an input `over` names replaces this one's of that name, field
    /// by field down to the rule base, and `null` removes one; a rule base's
    /// `rules` are replaced whole.
    pub fn merged(&self, over: &RuleSet) -> Result<RuleSet> {
        let source = over
            .files
            .last()
            .map(|f| f.label.clone())
            .unwrap_or_else(|| "<rules>".into());
        let mut doc = self.doc.clone();
        merge_doc(&mut doc, &over.doc);
        let mut set = Self::from_doc(doc, &source)?;
        // Quantities over did not touch keep where they came from.
        for (name, q) in &mut set.quantities {
            if !over.quantities.contains_key(name) {
                if let Some(mine) = self.quantities.get(name) {
                    q.source = mine.source.clone();
                }
            }
        }
        set.files = self.files.iter().chain(&over.files).cloned().collect();
        Ok(set)
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

/// One YAML document of `text`, merge keys (`<<`) worked out, its version
/// checked.
fn document(text: &str, source: &str) -> Result<Node> {
    let docs = parse_all(text).map_err(|e| Error::value(format!("{source}: {e}")))?;
    let [doc] = docs.as_slice() else {
        return Err(Error::value(format!(
            "{source}: expected one YAML document, got {}",
            docs.len()
        )));
    };
    let mut doc = doc.clone();
    doc.apply_merge()
        .map_err(|e| Error::value(format!("{source}: {e}")))?;
    let at = Where::new(source);
    let map = mapping(&doc, &at, &["version", "extends", "quantities"])?;
    match get(map, "version").and_then(Node::as_i64) {
        Some(VERSION) => Ok(doc),
        _ => Err(at.error(format!(
            "version must be {VERSION}, got {}",
            get(map, "version")
                .map(NodeExt::py_repr)
                .unwrap_or("None".into())
        ))),
    }
}

/// What a document `extends`: a reference, or a list of them.
fn extends_of(doc: &Node, at: &Where) -> Result<Vec<Reference>> {
    let Some(raw) = doc.get("extends").map(untag) else {
        return Ok(Vec::new());
    };
    let at = at.key("extends");
    let items: Vec<&Node> = match raw {
        Node::Sequence(items) => items.iter().collect(),
        other => vec![other],
    };
    let mut out = Vec::new();
    for (k, item) in items.into_iter().enumerate() {
        let at = at.index(k);
        out.push(match untag(item) {
            Node::String(s) if s.starts_with("https://") => Reference::Url {
                url: s.clone(),
                sha256: None,
            },
            Node::String(s) if s.starts_with("http://") => {
                return Err(at.error(format!("{s}: only https:// URLs are read")))
            }
            Node::String(s) if !s.trim().is_empty() => Reference::Path(s.clone()),
            Node::Mapping(_) if item.get("url").is_some() => {
                let map = mapping(item, &at, &["url", "sha256"])?;
                let url = get(map, "url").map(NodeExt::py_str).unwrap_or_default();
                if !url.starts_with("https://") {
                    return Err(at.error(format!("{url}: only https:// URLs are read")));
                }
                let sha256 = get(map, "sha256").map(|n| n.py_str().to_lowercase());
                if let Some(h) = &sha256 {
                    if h.len() != 64 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
                        return Err(at.error("sha256 must be 64 hex digits"));
                    }
                }
                Reference::Url { url, sha256 }
            }
            Node::Mapping(_) => {
                let map = mapping(item, &at, &["git", "rev", "path"])?;
                let field = |k: &str| get(map, k).map(NodeExt::py_str).unwrap_or_default();
                Reference::Git(
                    GitSource::new(field("git"), field("rev"), field("path"))
                        .map_err(|e| at.error(e))?,
                )
            }
            other => {
                return Err(at.error(format!(
                "expected a path, an https:// URL, {{url, sha256}} or {{git, rev, path}}, got {}",
                other.py_repr()
            )))
            }
        });
    }
    Ok(out)
}

/// `text` with every file it extends merged in under it, as one document:
/// the files extended in order, each over the last, then `text` over them.
fn compose(
    text: &str,
    source: &str,
    origin: &Origin,
    fetch: &mut dyn Fetch,
    stack: &mut Vec<String>,
    files: &mut Vec<FileRead>,
) -> Result<Node> {
    let mut doc = document(text, source)?;
    let at = Where::new(source);
    let mut base: Option<Node> = None;
    for reference in extends_of(&doc, &at)? {
        let next = origin
            .of(&reference)
            .map_err(|e| at.key("extends").error(e))?;
        let label = next.label();
        if stack.contains(&next.key()) || next.key() == origin.key() {
            return Err(at.key("extends").error(format!("{label} extends itself")));
        }
        if stack.len() >= MAX_EXTENDS_DEPTH {
            return Err(at.key("extends").error(format!(
                "extends nests deeper than {MAX_EXTENDS_DEPTH} files"
            )));
        }
        let text = fetch.fetch(&next).map_err(|e| at.key("extends").error(e))?;
        if let Reference::Url {
            sha256: Some(want), ..
        } = &reference
        {
            let got = hex::encode(Sha256::digest(text.as_bytes()));
            if &got != want {
                return Err(at
                    .key("extends")
                    .error(format!("{label}: its SHA-256 is {got}, not {want}")));
            }
        }
        stack.push(origin.key());
        let extended = compose(&text, &label, &next, fetch, stack, files)?;
        stack.pop();
        base = Some(match base {
            None => extended,
            Some(mut b) => {
                merge_doc(&mut b, &extended);
                b
            }
        });
    }
    if let Node::Mapping(map) = &mut doc {
        map.remove("extends");
    }
    files.push(FileRead {
        label: source.to_string(),
        sha256: hex::encode(Sha256::digest(text.as_bytes())),
    });
    Ok(match base {
        None => doc,
        Some(mut b) => {
            merge_doc(&mut b, &doc);
            b
        }
    })
}

/// Merge the rule document `over` into `base` (see [`RuleSet::merged`]).
fn merge_doc(base: &mut Node, over: &Node) {
    merge_level(base, over, &[("quantities", 0)]);
}

/// `over`'s entries into `base`'s, `depth` naming what the mapping holds:
/// 0 quantities, 1 rule bases, 2 inputs. Down to the rule base an entry
/// merges field by field; below it, it replaces.
fn merge_level(base: &mut Node, over: &Node, named: &[(&str, usize)]) {
    let (Node::Mapping(b), Node::Mapping(o)) = (base, untag(over)) else {
        return;
    };
    for (k, v) in o {
        let key = k.py_str();
        let Some((_, depth)) = named.iter().find(|(n, _)| *n == key) else {
            b.insert(k.clone(), v.clone());
            continue;
        };
        let Node::Mapping(entries) = untag(v) else {
            b.insert(k.clone(), v.clone());
            continue;
        };
        if !matches!(b.get(k), Some(Node::Mapping(_))) {
            b.insert(k.clone(), Node::Mapping(crate::yaml::Mapping::new()));
        }
        let Some(Node::Mapping(mine)) = b.get_mut(k) else {
            continue;
        };
        for (name, entry) in entries {
            if matches!(untag(entry), Node::Null) {
                mine.remove(name);
                continue;
            }
            let fields: &[(&str, usize)] = match depth {
                0 => &[("rule_bases", 1)],
                1 => &[("inputs", 2)],
                _ => {
                    mine.insert(name.clone(), entry.clone());
                    continue;
                }
            };
            match mine.get_mut(name) {
                Some(existing @ Node::Mapping(_)) => merge_level(existing, entry, fields),
                _ => {
                    mine.insert(name.clone(), entry.clone());
                }
            }
        }
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
    for x in input.check_points() {
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
        let merged = base.merged(&over).unwrap();
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
    fn a_gap_between_the_evenly_spaced_points_is_found() {
        // Nothing covers 33.3, which no evenly spaced point falls on.
        let text = GRIP
            .replace(
                "dry: {trapezoid: [0, 0, 0, 50]}",
                "dry: {triangle: [0, 0, 33.3]}",
            )
            .replace(
                "wet: {triangle: [0, 50, 100]}",
                "wet: {triangle: [33.3, 100, 100]}",
            )
            .replace(
                "flooded: {trapezoid: [50, 100, 100, 100]}",
                "flooded: {triangle: [99, 100, 100]}",
            );
        let e = RuleSet::parse(&text, "grip.yaml").unwrap_err().to_string();
        assert!(e.contains("no term covers water = 33.3,"), "{e}");
        // Covered there, but by a term no rule names: no rule fires.
        let text = text.replace(
            "flooded: {triangle: [99, 100, 100]}",
            "flooded: {triangle: [33, 33.3, 33.6]}",
        );
        let text = text.replace(
            "          - if: (water IS flooded)\n            then: {low: 0.45, high: 0.65}\n",
            "",
        );
        let e = RuleSet::parse(&text, "grip.yaml").unwrap_err().to_string();
        assert!(e.contains("no rule fires at water=33.3"), "{e}");
    }

    #[test]
    fn a_rule_base_too_wide_to_check_is_refused_at_once() {
        let mut inputs = String::new();
        let mut clause = Vec::new();
        for k in 0..25 {
            inputs.push_str(&format!(
                "          x{k}:\n            universe: [0, 1]\n            terms: {{any: {{trapezoid: [0, 0, 1, 1]}}}}\n"
            ));
            clause.push(format!("(x{k} IS any)"));
        }
        let text = format!(
            "version: 1\nquantities:\n  q:\n    rule_bases:\n      wide:\n        inputs:\n{inputs}        rules:\n          - if: {}\n            then: {{v: 1}}\n",
            clause.join(" AND ")
        );
        let started = std::time::Instant::now();
        let e = RuleSet::parse(&text, "wide.yaml").unwrap_err().to_string();
        assert!(
            e.contains("split it into rule bases reading fewer inputs"),
            "{e}"
        );
        assert!(started.elapsed().as_secs() < 1);
    }

    const OVER_WATER: &str = r#"
version: 1
extends: base.yaml
quantities:
  grip:
    rule_bases:
      water:
        rules:
          - if: (water IS dry)
            then: {low: 0.95, high: 1.05}
          - if: (water IS wet)
            then: {low: 0.60, high: 0.80}
          - if: (water IS flooded)
            then: {low: 0.30, high: 0.50}
"#;

    fn files(dir: &Path, entries: &[(&str, &str)]) {
        for (name, text) in entries {
            let path = dir.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
    }

    fn load(path: &Path) -> Result<RuleSet> {
        RuleSet::load(&Origin::File(path.to_path_buf()), &mut Files { url: None })
    }

    #[test]
    fn a_file_extends_another_writing_only_what_it_changes() {
        let dir = tempfile::tempdir().unwrap();
        files(
            dir.path(),
            &[
                ("base.yaml", GRIP),
                (
                    "vehicles/mini.yaml",
                    &OVER_WATER.replace("base.yaml", "../base.yaml"),
                ),
            ],
        );
        let rules = load(&dir.path().join("vehicles/mini.yaml")).unwrap();
        // The inputs are base.yaml's, the rules mini.yaml's.
        assert!(close(
            at(&rules, "grip", &[("water", 100.0)]).unwrap(),
            (0.30, 0.50)
        ));
        assert!(close(
            at(&rules, "grip", &[("water", 25.0)]).unwrap(),
            (0.775, 0.925)
        ));
        let labels: Vec<&str> = rules.files.iter().map(|f| f.label.as_str()).collect();
        assert!(
            labels[0].ends_with("base.yaml")
                && !labels[0].contains("..")
                && labels[1].ends_with("mini.yaml"),
            "{labels:?}"
        );
        assert_eq!(
            rules.files[0].sha256,
            hex::encode(Sha256::digest(GRIP.as_bytes()))
        );
    }

    #[test]
    fn extending_merges_down_to_the_rule_base_and_null_removes() {
        let dir = tempfile::tempdir().unwrap();
        let two = GRIP.replace("    rule_bases:\n", "    combine: max\n    rule_bases:\n")
            + &GRIP
                .replace(
                    "version: 1\nquantities:\n  grip:\n    rule_bases:\n      water:",
                    "      water2:",
                )
                .replace("\nversion: 1\nquantities:\n  grip:\n    rule_bases:\n", "");
        let child = "version: 1\nextends: base.yaml\nquantities:\n  grip:\n    combine: min\n    rule_bases:\n      water2: null\n      water:\n        inputs:\n          water:\n            universe: [0, 100]\n            default: 100\n            terms:\n              dry: {trapezoid: [0, 0, 0, 50]}\n              wet: {triangle: [0, 50, 100]}\n              flooded: {trapezoid: [50, 100, 100, 100]}\n";
        files(dir.path(), &[("base.yaml", &two), ("child.yaml", child)]);
        let base = load(&dir.path().join("base.yaml")).unwrap();
        assert_eq!(base.quantity("grip").unwrap().rule_bases.len(), 2);
        let rules = load(&dir.path().join("child.yaml")).unwrap();
        let q = rules.quantity("grip").unwrap();
        assert_eq!(q.combine, Combine::Min);
        assert_eq!(q.rule_bases.len(), 1);
        // The input replaced (a default now), the rules kept.
        assert!(close(at(&rules, "grip", &[]).unwrap(), (0.45, 0.65)));
    }

    #[test]
    fn a_file_naming_only_its_change_is_read_over_rules_in_hand() {
        let dir = tempfile::tempdir().unwrap();
        files(
            dir.path(),
            &[("run.yaml", &OVER_WATER.replace("extends: base.yaml\n", ""))],
        );
        let origin = Origin::File(dir.path().join("run.yaml"));
        // On its own it is no whole set: water has no inputs.
        let e = load(&dir.path().join("run.yaml")).unwrap_err().to_string();
        assert!(e.contains("inputs: missing"), "{e}");
        let base = RuleSet::parse(GRIP, "backend.yaml").unwrap();
        let rules = base.load_over(&origin, &mut Files { url: None }).unwrap();
        assert!(close(
            at(&rules, "grip", &[("water", 100.0)]).unwrap(),
            (0.30, 0.50)
        ));
        assert_eq!(rules.files.len(), 2);
    }

    #[test]
    fn a_chain_of_files_and_a_list_apply_in_order_and_a_cycle_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        files(
            dir.path(),
            &[
                ("base.yaml", GRIP),
                ("a.yaml", &OVER_WATER.replace("0.30", "0.31")),
                ("b.yaml", &OVER_WATER.replace("0.30", "0.32")),
                (
                    "both.yaml",
                    "version: 1\nextends: [a.yaml, b.yaml]\nquantities: {}\n",
                ),
                ("c1.yaml", "version: 1\nextends: c2.yaml\nquantities: {}\n"),
                ("c2.yaml", "version: 1\nextends: c1.yaml\nquantities: {}\n"),
            ],
        );
        let rules = load(&dir.path().join("both.yaml")).unwrap();
        assert!(close(
            at(&rules, "grip", &[("water", 100.0)]).unwrap(),
            (0.32, 0.50)
        ));
        let e = load(&dir.path().join("c1.yaml")).unwrap_err().to_string();
        assert!(e.contains("extends itself"), "{e}");
        let e = load(&dir.path().join("nowhere.yaml"))
            .unwrap_err()
            .to_string();
        assert!(e.contains("cannot read rules"), "{e}");
    }

    #[test]
    fn a_cycle_through_another_name_of_the_file_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        files(
            dir.path(),
            &[
                (
                    "a.yaml",
                    "version: 1\nextends: sub/../a.yaml\nquantities: {}\n",
                ),
                ("sub/keep", ""),
                (
                    "deep/b.yaml",
                    "version: 1\nextends: ./../deep/./b.yaml\nquantities: {}\n",
                ),
            ],
        );
        for f in ["a.yaml", "deep/b.yaml"] {
            let e = load(&dir.path().join(f)).unwrap_err().to_string();
            assert!(e.contains("extends itself"), "{f}: {e}");
        }
    }

    #[test]
    fn a_chain_deeper_than_the_limit_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let mut entries = Vec::new();
        for k in 0..=MAX_EXTENDS_DEPTH + 1 {
            entries.push((
                format!("f{k}.yaml"),
                format!("version: 1\nextends: f{}.yaml\nquantities: {{}}\n", k + 1),
            ));
        }
        entries.push((format!("f{}.yaml", MAX_EXTENDS_DEPTH + 2), GRIP.to_string()));
        let refs: Vec<(&str, &str)> = entries
            .iter()
            .map(|(a, b)| (a.as_str(), b.as_str()))
            .collect();
        files(dir.path(), &refs);
        let e = load(&dir.path().join("f0.yaml")).unwrap_err().to_string();
        assert!(e.contains("nests deeper than"), "{e}");
    }

    #[test]
    fn where_a_bell_underflows_is_checked_too() {
        // All four terms are zero at 33.5: the narrow bells underflow there.
        let text = GRIP
            .replace("dry: {trapezoid: [0, 0, 0, 50]}", "dry: {triangle: [0, 0, 33.4]}\n              b1: {gaussian: [33.4, 0.0001]}\n              b2: {gaussian: [33.8, 0.006]}")
            .replace("wet: {triangle: [0, 50, 100]}", "wet: {triangle: [33.8, 100, 100]}")
            .replace("flooded: {trapezoid: [50, 100, 100, 100]}", "flooded: {triangle: [99, 100, 100]}");
        let e = RuleSet::parse(&text, "grip.yaml").unwrap_err().to_string();
        assert!(e.contains("no term covers water"), "{e}");
    }

    #[test]
    fn a_url_is_read_by_the_host_and_its_relative_paths_against_it() {
        let mut seen = Vec::new();
        let mut url = |u: &str| -> Result<String> {
            seen.push(u.to_string());
            match u {
                "https://example.com/rules/base.yaml" => Ok(GRIP.to_string()),
                "https://example.com/rules/vehicles/mini.yaml" => {
                    Ok(OVER_WATER.replace("base.yaml", "../base.yaml"))
                }
                _ => Err(Error::value("404")),
            }
        };
        let rules = RuleSet::load(
            &Origin::Url("https://example.com/rules/vehicles/mini.yaml".into()),
            &mut Files {
                url: Some(&mut url),
            },
        )
        .unwrap();
        assert!(close(
            at(&rules, "grip", &[("water", 100.0)]).unwrap(),
            (0.30, 0.50)
        ));
        assert_eq!(seen[1], "https://example.com/rules/base.yaml");
        // Without a way to fetch, a URL is an error; plain http is refused.
        let text = OVER_WATER.replace("base.yaml", "https://example.com/rules/base.yaml");
        let e = RuleSet::parse_with(&text, "x.yaml", &Origin::Text, &mut Files { url: None })
            .unwrap_err()
            .to_string();
        assert!(e.contains("no way to fetch a URL"), "{e}");
        let e = RuleSet::parse(
            &OVER_WATER.replace("base.yaml", "http://example.com/b.yaml"),
            "x.yaml",
        )
        .unwrap_err()
        .to_string();
        assert!(e.contains("only https:// URLs are read"), "{e}");
    }

    #[test]
    fn a_pinned_url_must_have_its_digest() {
        let digest = hex::encode(Sha256::digest(GRIP.as_bytes()));
        let given = |pin: &str| {
            let text = OVER_WATER.replace(
                "extends: base.yaml",
                &format!("extends: {{url: https://example.com/base.yaml, sha256: {pin}}}"),
            );
            let mut map = IndexMap::new();
            map.insert(
                "https://example.com/base.yaml".to_string(),
                GRIP.to_string(),
            );
            RuleSet::parse_with(&text, "x.yaml", &Origin::Text, &mut Given(map))
        };
        assert!(given(&digest).is_ok());
        let e = given(&"0".repeat(64)).unwrap_err().to_string();
        assert!(e.contains(&format!("its SHA-256 is {digest}")), "{e}");
    }

    #[test]
    fn the_references_are_what_a_browser_fetches_ahead() {
        let text = OVER_WATER.replace(
            "extends: base.yaml",
            "extends: [base.yaml, https://x.org/a/b.yaml]",
        );
        let refs = RuleSet::references(
            &text,
            "v.yaml",
            &Origin::Url("https://example.com/r/v.yaml".into()),
        )
        .unwrap();
        assert_eq!(
            refs.iter().map(Origin::label).collect::<Vec<_>>(),
            vec!["https://example.com/r/base.yaml", "https://x.org/a/b.yaml"]
        );
    }

    #[test]
    fn a_git_source_is_read_at_its_revision() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        files(&repo, &[("weather/base.yaml", GRIP)]);
        let git = |args: &[&str]| {
            let ok = std::process::Command::new("git")
                .args([
                    "-c",
                    "user.name=t",
                    "-c",
                    "user.email=t@t",
                    "-c",
                    "commit.gpgsign=false",
                ])
                .args(args)
                .current_dir(&repo)
                .output()
                .unwrap()
                .status
                .success();
            assert!(ok, "git {args:?}");
        };
        git(&["init", "--quiet"]);
        git(&["add", "."]);
        git(&["commit", "--quiet", "-m", "base"]);
        git(&["tag", "v1"]);
        std::env::set_var(crate::sources::CACHE_ENV, dir.path().join("cache"));
        let text = OVER_WATER.replace(
            "extends: base.yaml",
            &format!(
                "extends: {{git: {}, rev: v1, path: weather/base.yaml}}",
                repo.display()
            ),
        );
        let rules =
            RuleSet::parse_with(&text, "x.yaml", &Origin::Text, &mut Files { url: None }).unwrap();
        assert!(close(
            at(&rules, "grip", &[("water", 100.0)]).unwrap(),
            (0.30, 0.50)
        ));
        assert!(rules.files[0].label.ends_with("@v1:weather/base.yaml"));
    }

    #[test]
    fn merge_keys_share_part_of_a_mapping() {
        let text = GRIP.replace(
            "          water:\n            universe: [0, 100]\n",
            "          water:\n            <<: &universe {universe: [0, 100]}\n",
        );
        let rules = RuleSet::parse(&text, "grip.yaml").unwrap();
        assert!(close(
            at(&rules, "grip", &[("water", 25.0)]).unwrap(),
            (0.775, 0.975)
        ));
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
