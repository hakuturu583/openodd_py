//! An ODD: the attributes it is described in, and the modules that bound it.
//!
//! The model follows ASAM OpenODD 1.0 (chapters 6 and 7):
//!
//! * An **attribute** is a taxonomy concept that can be measured. Here it also
//!   has a **probe** -- which the backend reads (see the Python `Backend`), or
//!   which is derived from the other attributes -- and **buckets**, which make
//!   it a cover item.
//! * A **module** holds when its include section holds and its exclude does
//!   not (`MODULE === INCLUDE AND (NOT EXCLUDE)`). At most one include and one
//!   exclude section. A condition tests one attribute, groups others, or refers
//!   to a module or a label. A label holds when any active module declaring it
//!   holds.
//! * An **inactive** module is ignored: a condition referring to it drops out
//!   of its section; a section left empty is absent.
//! * The **roots** are the root candidates (by default every module) no other
//!   module refers to. The ODD holds when its active roots hold.
//!
//! Missing values follow OpenODD's missing-value semantics: a missing value
//! does not by itself put a situation outside the ODD. Conditions are
//! evaluated three-valued; a verdict unknown only because values are missing
//! counts as inside, and is flagged as *assumed*.
//!
//! The same evaluation decides which buckets lie outside the ODD: the bucket
//! stands for the attribute's value and every other attribute is left open; a
//! bucket is outside when the ODD fails whatever the others are.

use std::collections::{BTreeSet, HashMap, HashSet};

use indexmap::IndexMap;
use serde_json::{json, Value as Json};

use crate::error::{Error, Result};
use crate::items::{Buckets, CoverGroup, CoverItem, Criteria, HOLDS, TICK};
use crate::value::Value;

// ---------------------------------------------------------------------------
// Three-valued logic
// ---------------------------------------------------------------------------

/// What a condition, a module or a label evaluates to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Truth {
    True,
    False,
    /// A value it needs is missing.
    Unknown,
    /// An inactive module, or a section with nothing left in it.
    Inactive,
}

impl Truth {
    fn from_bool(b: bool) -> Self {
        if b {
            Truth::True
        } else {
            Truth::False
        }
    }

    /// `None` for unknown, as the three-valued verdict is written.
    pub fn as_option(self) -> Option<bool> {
        match self {
            Truth::True => Some(true),
            Truth::False => Some(false),
            _ => None,
        }
    }
}

fn and(values: impl IntoIterator<Item = Truth>) -> Truth {
    let mut unknown = false;
    for v in values {
        match v {
            Truth::Inactive => continue,
            Truth::False => return Truth::False,
            Truth::Unknown => unknown = true,
            Truth::True => {}
        }
    }
    if unknown {
        Truth::Unknown
    } else {
        Truth::True
    }
}

fn or(values: impl IntoIterator<Item = Truth>) -> Truth {
    let mut unknown = false;
    for v in values {
        match v {
            Truth::Inactive => continue,
            Truth::True => return Truth::True,
            Truth::Unknown => unknown = true,
            Truth::False => {}
        }
    }
    if unknown {
        Truth::Unknown
    } else {
        Truth::False
    }
}

fn not(v: Truth) -> Truth {
    match v {
        Truth::True => Truth::False,
        Truth::False => Truth::True,
        other => other,
    }
}

// ---------------------------------------------------------------------------
// Predicates on one value
// ---------------------------------------------------------------------------

/// What a leaf condition says of a value.
#[derive(Debug, Clone, PartialEq)]
pub enum Predicate {
    /// One of the values (an OpenODD list).
    InSet {
        labels: BTreeSet<String>,
        numbers: Vec<f64>,
    },
    /// A bound or a range.
    Interval {
        low: f64,
        high: f64,
        include_low: bool,
        include_high: bool,
    },
    /// Equal to a value.
    Equal(Value),
}

fn fmt_g6(x: f64) -> String {
    crate::value::format_g(x, 6)
}

impl Predicate {
    fn interval_holds(low: f64, high: f64, il: bool, ih: bool, x: f64) -> bool {
        if x.is_nan() {
            return false;
        }
        (x > low || (il && x == low)) && (x < high || (ih && x == high))
    }

    /// Whether `value` (present) satisfies it; `None` when it cannot be read
    /// as the predicate needs (then the condition is false).
    pub fn holds(&self, value: &Value) -> Option<bool> {
        match self {
            Predicate::InSet { labels, numbers } => {
                if let Some(n) = value.number() {
                    if numbers.contains(&n) {
                        return Some(true);
                    }
                }
                Some(labels.contains(&value.label()))
            }
            Predicate::Interval {
                low,
                high,
                include_low,
                include_high,
            } => {
                let x = value.as_float()?;
                Some(Self::interval_holds(
                    *low,
                    *high,
                    *include_low,
                    *include_high,
                    x,
                ))
            }
            Predicate::Equal(expected) => {
                if let Some(e) = expected.number() {
                    return Some(value.number() == Some(e));
                }
                Some(value.label() == expected.label())
            }
        }
    }

    /// Whether `[low, high)` (`]` when closed) lies within: `None`, partly.
    fn on_range(&self, low: f64, high: f64, closed: bool) -> Option<bool> {
        let (l, h, il, ih) = match self {
            Predicate::Interval {
                low,
                high,
                include_low,
                include_high,
            } => (*low, *high, *include_low, *include_high),
            Predicate::Equal(v) if v.number().is_some() => {
                let p = v.number().unwrap();
                (p, p, true, true)
            }
            _ => return None,
        };
        let holds = |x: f64| Self::interval_holds(l, h, il, ih, x);
        if holds(low) && (if closed { holds(high) } else { high <= h }) {
            return Some(true);
        }
        let below = high < l || (high == l && (!closed || !il));
        let above = low > h || (low == h && !ih);
        if below || above {
            Some(false)
        } else {
            None
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Predicate::InSet { labels, .. } => {
                format!(
                    "in [{}]",
                    labels.iter().cloned().collect::<Vec<_>>().join(", ")
                )
            }
            Predicate::Interval {
                low,
                high,
                include_low,
                include_high,
            } => {
                if *low == f64::NEG_INFINITY {
                    format!(
                        "{} {}",
                        if *include_high { "<=" } else { "<" },
                        fmt_g6(*high)
                    )
                } else if *high == f64::INFINITY {
                    format!("{} {}", if *include_low { ">=" } else { ">" }, fmt_g6(*low))
                } else {
                    format!(
                        "in {}{} .. {}{}",
                        if *include_low { '[' } else { '(' },
                        fmt_g6(*low),
                        fmt_g6(*high),
                        if *include_high { ']' } else { ')' }
                    )
                }
            }
            Predicate::Equal(v) => format!("== {}", v.label()),
        }
    }
}

// ---------------------------------------------------------------------------
// Conditions
// ---------------------------------------------------------------------------

/// A condition of a module.
#[derive(Debug, Clone, PartialEq)]
pub enum Condition {
    /// A predicate on one attribute.
    Leaf {
        attribute: String,
        predicate: Predicate,
    },
    /// OpenODD's `x: unknown`: true exactly when the value is missing.
    Missing { attribute: String },
    /// All (`AND`) or any (`OR`) of the children.
    Group { all: bool, children: Vec<Condition> },
    /// That a module or a label holds (or not).
    ModuleRef { name: String, holds: bool },
}

impl Condition {
    pub fn all_of(children: Vec<Condition>) -> Result<Self> {
        if children.is_empty() {
            return Err(Error::value("all_of() needs at least one condition"));
        }
        Ok(Condition::Group {
            all: true,
            children,
        })
    }

    pub fn any_of(children: Vec<Condition>) -> Result<Self> {
        if children.is_empty() {
            return Err(Error::value("any_of() needs at least one condition"));
        }
        Ok(Condition::Group {
            all: false,
            children,
        })
    }

    pub fn module_holds(name: impl Into<String>, holds: bool) -> Result<Self> {
        let name = name.into();
        if name.is_empty() {
            return Err(Error::value("module_holds(): name must not be empty"));
        }
        Ok(Condition::ModuleRef { name, holds })
    }

    fn evaluate(&self, values: &Values, truth: &HashMap<String, Truth>, items: &Items) -> Truth {
        match self {
            Condition::Leaf {
                attribute,
                predicate,
            } => {
                let value = values.get(attribute).unwrap_or(&Value::Missing);
                match value {
                    Value::Open => Truth::Unknown,
                    v if v.is_missing() => Truth::Unknown,
                    Value::Bucket(i) => {
                        on_bucket(predicate, items.get(attribute.as_str()).copied(), *i)
                    }
                    v => match predicate.holds(v) {
                        Some(b) => Truth::from_bool(b),
                        // present, but not a value the condition can hold for
                        None => Truth::False,
                    },
                }
            }
            Condition::Missing { attribute } => {
                match values.get(attribute).unwrap_or(&Value::Missing) {
                    Value::Open => Truth::Unknown,
                    v => Truth::from_bool(v.is_missing()),
                }
            }
            Condition::Group { all, children } => {
                let results: Vec<Truth> = children
                    .iter()
                    .map(|c| c.evaluate(values, truth, items))
                    .collect();
                if results.iter().all(|r| *r == Truth::Inactive) {
                    return Truth::Inactive;
                }
                if *all {
                    and(results)
                } else {
                    or(results)
                }
            }
            Condition::ModuleRef { name, holds } => match truth.get(name) {
                Some(Truth::Inactive) => Truth::Inactive,
                Some(Truth::True) => Truth::from_bool(*holds),
                Some(Truth::False) => Truth::from_bool(!*holds),
                _ => Truth::Unknown,
            },
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Condition::Leaf {
                attribute,
                predicate,
            } => {
                format!("{attribute} {}", predicate.describe())
            }
            Condition::Missing { attribute } => format!("{attribute} is unknown"),
            Condition::Group { all, children } => {
                let joiner = if *all { " and " } else { " or " };
                format!(
                    "({})",
                    children
                        .iter()
                        .map(Condition::describe)
                        .collect::<Vec<_>>()
                        .join(joiner)
                )
            }
            Condition::ModuleRef { name, holds } => format!("{name} is {holds}"),
        }
    }

    /// The modules and labels it refers to.
    pub fn references(&self, out: &mut BTreeSet<String>) {
        match self {
            Condition::ModuleRef { name, .. } => {
                out.insert(name.clone());
            }
            Condition::Group { children, .. } => children.iter().for_each(|c| c.references(out)),
            _ => {}
        }
    }

    /// The attributes it tests, in order.
    pub fn attributes(&self, out: &mut Vec<String>) {
        match self {
            Condition::Leaf { attribute, .. } | Condition::Missing { attribute } => {
                out.push(attribute.clone())
            }
            Condition::Group { children, .. } => children.iter().for_each(|c| c.attributes(out)),
            Condition::ModuleRef { .. } => {}
        }
    }

    /// Every leaf predicate, with its attribute.
    pub fn leaves<'a>(&'a self, out: &mut Vec<(&'a str, &'a Predicate)>) {
        match self {
            Condition::Leaf {
                attribute,
                predicate,
            } => out.push((attribute, predicate)),
            Condition::Group { children, .. } => children.iter().for_each(|c| c.leaves(out)),
            _ => {}
        }
    }
}

fn on_bucket(predicate: &Predicate, item: Option<&CoverItem>, i: usize) -> Truth {
    let Some(item) = item else {
        return Truth::Unknown;
    };
    if !item.numeric() {
        return match item.values.get(i).and_then(|v| predicate.holds(v)) {
            Some(b) => Truth::from_bool(b),
            None => Truth::Unknown,
        };
    }
    let last = i == item.labels.len() - 1;
    match predicate.on_range(item.edges[i], item.edges[i + 1], last) {
        Some(b) => Truth::from_bool(b),
        None => Truth::Unknown,
    }
}

/// Attribute values by name.
pub type Values = HashMap<String, Value>;
type Items<'a> = HashMap<&'a str, &'a CoverItem>;

// ---------------------------------------------------------------------------
// Attributes
// ---------------------------------------------------------------------------

/// Where an attribute's value comes from.
#[derive(Debug, Clone, PartialEq)]
pub enum Probe {
    /// Nothing measures it: always missing.
    Missing,
    /// Read by the backend, under this key (`probe:` name, `measure:` key, ...).
    External { kind: String, spec: String },
    /// An OpenODD categorical defined by expressions: the first literal whose
    /// expressions hold.
    Derived(Vec<(String, Condition)>),
}

/// A measurable taxonomy concept.
#[derive(Debug, Clone, PartialEq)]
pub struct Attribute {
    pub name: String,
    pub unit: String,
    pub text: String,
    pub probe: Probe,
    pub criteria: Criteria,
    /// Its cover item (`odd.<name>`), when it has buckets.
    pub item: Option<CoverItem>,
}

impl Attribute {
    pub fn new(
        name: impl Into<String>,
        probe: Probe,
        unit: impl Into<String>,
        text: impl Into<String>,
        criteria: Criteria,
    ) -> Result<Self> {
        let name = name.into();
        if name.is_empty() {
            return Err(Error::value("OddAttribute: name must not be empty"));
        }
        criteria.check(&format!("OddAttribute({name})"), TICK)?;
        Ok(Attribute {
            name,
            unit: unit.into(),
            text: text.into(),
            probe,
            criteria,
            item: None,
        })
    }

    /// Give the attribute buckets: its cover item.
    pub fn set_buckets(&mut self, buckets: Buckets) -> Result<()> {
        let mut item = CoverItem::new(
            format!("odd.{}", self.name),
            buckets,
            TICK,
            CoverGroup::Odd,
            self.criteria.clone(),
        )?;
        item.unit = self.unit.clone();
        item.text = self.text.clone();
        self.item = Some(item);
        Ok(())
    }

    pub fn derived(&self) -> bool {
        matches!(self.probe, Probe::Derived(_))
    }
}

// ---------------------------------------------------------------------------
// Modules
// ---------------------------------------------------------------------------

/// A section of a module: all (`*_AND`) or any (`*_OR`) of its conditions.
#[derive(Debug, Clone, PartialEq)]
pub struct Section {
    pub all: bool,
    pub conditions: Vec<Condition>,
}

impl Section {
    fn evaluate(&self, values: &Values, truth: &HashMap<String, Truth>, items: &Items) -> Truth {
        let results: Vec<Truth> = self
            .conditions
            .iter()
            .map(|c| c.evaluate(values, truth, items))
            .collect();
        if results.iter().all(|r| *r == Truth::Inactive) {
            return Truth::Inactive;
        }
        if self.all {
            and(results)
        } else {
            or(results)
        }
    }
}

/// A named rule: it holds when its include section holds and its exclude does not.
#[derive(Debug, Clone, PartialEq)]
pub struct Module {
    pub name: String,
    pub include: Option<Section>,
    pub exclude: Option<Section>,
    pub labels: Vec<String>,
    pub active: bool,
    pub text: String,
    /// Its cover item (`odd.situation.<name>`) when covered as a situation.
    pub item: Option<CoverItem>,
    /// The module's `METADATA` (OpenODD 6.4.6.3), as written.
    pub metadata: IndexMap<String, Json>,
}

impl Module {
    pub fn new(
        name: impl Into<String>,
        include: Option<Section>,
        exclude: Option<Section>,
    ) -> Result<Self> {
        let name = name.into();
        if name.is_empty() {
            return Err(Error::value("OddModule: name must not be empty"));
        }
        Ok(Module {
            name,
            include: include.filter(|s| !s.conditions.is_empty()),
            exclude: exclude.filter(|s| !s.conditions.is_empty()),
            labels: Vec::new(),
            active: true,
            text: String::new(),
            item: None,
            metadata: IndexMap::new(),
        })
    }

    /// Cover the module as a situation, with these criteria.
    pub fn as_situation(&mut self, criteria: Criteria) -> Result<()> {
        let mut item = CoverItem::new(
            format!("odd.situation.{}", self.name),
            Buckets::Values(vec![Value::Str(HOLDS.into())]),
            TICK,
            CoverGroup::Situation,
            criteria,
        )?;
        item.text = self.text.clone();
        self.item = Some(item);
        Ok(())
    }

    pub fn situation(&self) -> bool {
        self.item.is_some()
    }

    pub fn conditions(&self) -> impl Iterator<Item = &Condition> {
        self.include
            .iter()
            .chain(self.exclude.iter())
            .flat_map(|s| s.conditions.iter())
    }

    fn evaluate(&self, values: &Values, truth: &HashMap<String, Truth>, items: &Items) -> Truth {
        let include = match self
            .include
            .as_ref()
            .map(|s| s.evaluate(values, truth, items))
        {
            None | Some(Truth::Inactive) => Truth::True, // an include with nothing left is absent
            Some(t) => t,
        };
        let exclude = match self
            .exclude
            .as_ref()
            .map(|s| s.evaluate(values, truth, items))
        {
            None | Some(Truth::Inactive) => Truth::False,
            Some(t) => t,
        };
        and([include, not(exclude)])
    }

    pub fn describe(&self) -> Json {
        let mut out = serde_json::Map::new();
        out.insert("name".into(), json!(self.name));
        out.insert("text".into(), json!(self.text));
        out.insert("active".into(), json!(self.active));
        for (kind, section) in [("include", &self.include), ("exclude", &self.exclude)] {
            if let Some(s) = section {
                out.insert(
                    format!("{kind}_{}", if s.all { "and" } else { "or" }),
                    json!(s
                        .conditions
                        .iter()
                        .map(Condition::describe)
                        .collect::<Vec<_>>()),
                );
            }
        }
        if !self.labels.is_empty() {
            out.insert("labels".into(), json!(self.labels));
        }
        if let Some(item) = &self.item {
            out.insert(
                "situation".into(),
                json!({
                    "target": crate::items::number_json(item.criteria.target),
                    "cover_by": item.criteria.cover_by,
                    "min_stay": item.criteria.min_stay,
                }),
            );
        }
        Json::Object(out)
    }
}

// ---------------------------------------------------------------------------
// The ODD
// ---------------------------------------------------------------------------

/// The ODD's verdict on one set of values.
#[derive(Debug, Clone, PartialEq)]
pub struct Verdict {
    /// Whether the values are inside the ODD (missing values do not put them out).
    pub inside: bool,
    /// Inside only because values were missing.
    pub assumed: bool,
    /// Every module's verdict, in the modules' order.
    pub modules: IndexMap<String, Truth>,
}

/// An ODD: its attributes and the modules that bound it.
#[derive(Debug, Clone)]
pub struct Odd {
    pub name: String,
    pub text: String,
    pub attributes: Vec<Attribute>,
    pub modules: Vec<Module>,
    pub roots: Vec<String>,
    order: Vec<usize>,
    label_modules: IndexMap<String, Vec<String>>,
    labels_after: HashMap<String, Vec<String>>,
    outside: IndexMap<String, Vec<String>>,
}

impl Odd {
    /// Build an ODD, checking that its modules make one.
    ///
    /// `roots`: the root candidates (those no other module refers to are the
    /// roots; all of them if every one is referred to); `None` takes every
    /// module that is not a situation.
    pub fn new(
        name: impl Into<String>,
        attributes: Vec<Attribute>,
        modules: Vec<Module>,
        roots: Option<Vec<String>>,
        text: impl Into<String>,
    ) -> Result<Self> {
        let name = name.into();
        if name.is_empty() {
            return Err(Error::value("OddDefinition: name must not be empty"));
        }
        let attr_names: Vec<String> = attributes.iter().map(|a| a.name.clone()).collect();
        let dup = dups(&attr_names);
        if !dup.is_empty() {
            return Err(Error::value(format!(
                "ODD {name}: attributes named more than once: {}",
                py_list(&dup)
            )));
        }
        let module_names: Vec<String> = modules.iter().map(|m| m.name.clone()).collect();
        let dup = dups(&module_names);
        if !dup.is_empty() {
            return Err(Error::value(format!(
                "ODD {name}: modules named more than once: {}",
                py_list(&dup)
            )));
        }
        let by_module: HashSet<&str> = module_names.iter().map(String::as_str).collect();
        let known: HashSet<&str> = attr_names.iter().map(String::as_str).collect();
        let mut label_modules: IndexMap<String, Vec<String>> = IndexMap::new();
        for m in &modules {
            for label in &m.labels {
                if by_module.contains(label.as_str()) || known.contains(label.as_str()) {
                    return Err(Error::value(format!(
                        "ODD {name}: label '{label}' is also a module's or an attribute's name"
                    )));
                }
                label_modules
                    .entry(label.clone())
                    .or_default()
                    .push(m.name.clone());
            }
        }
        if let Some(roots) = &roots {
            let unknown: Vec<String> = roots
                .iter()
                .filter(|r| !by_module.contains(r.as_str()))
                .cloned()
                .collect();
            if !unknown.is_empty() {
                return Err(Error::value(format!(
                    "ODD {name}: no root module {}",
                    py_list(&unknown)
                )));
            }
        }
        for m in &modules {
            for c in m.conditions() {
                let mut tested = Vec::new();
                c.attributes(&mut tested);
                for a in tested {
                    if !known.contains(a.as_str()) {
                        return Err(Error::value(format!(
                            "ODD {name}: module {} tests {a}, which is not one of the ODD's attributes",
                            m.name
                        )));
                    }
                }
                let mut refs = BTreeSet::new();
                c.references(&mut refs);
                for r in refs {
                    if !by_module.contains(r.as_str()) && !label_modules.contains_key(&r) {
                        return Err(Error::value(format!(
                            "ODD {name}: module {} refers to '{r}', which is neither a module nor a label",
                            m.name
                        )));
                    }
                }
            }
        }
        let mut odd = Odd {
            name,
            text: text.into(),
            attributes,
            modules,
            roots: Vec::new(),
            order: Vec::new(),
            label_modules,
            labels_after: HashMap::new(),
            outside: IndexMap::new(),
        };
        odd.order = odd.evaluation_order()?;
        let mut last: IndexMap<String, String> = IndexMap::new();
        for &i in &odd.order {
            for label in &odd.modules[i].labels {
                last.insert(label.clone(), odd.modules[i].name.clone());
            }
        }
        for (label, module) in last {
            odd.labels_after.entry(module).or_default().push(label);
        }
        // What a bound refers to is not a root. A situation's references do not
        // count: it is what to drive, and a bound it tests must still bound the ODD.
        let mut referenced = HashSet::new();
        for m in odd.modules.iter().filter(|m| !m.situation()) {
            referenced.extend(odd.depends_on(m));
        }
        odd.roots = match roots {
            Some(r) if !r.is_empty() => {
                let kept: Vec<String> = r
                    .iter()
                    .filter(|x| !referenced.contains(*x))
                    .cloned()
                    .collect();
                if kept.is_empty() {
                    r
                } else {
                    kept
                }
            }
            _ => odd
                .modules
                .iter()
                .filter(|m| !referenced.contains(&m.name) && !m.situation())
                .map(|m| m.name.clone())
                .collect(),
        };
        odd.outside = odd.outside_buckets_all();
        Ok(odd)
    }

    fn module_index(&self, name: &str) -> Option<usize> {
        self.modules.iter().position(|m| m.name == name)
    }

    /// The modules `module` refers to, by name or through a label.
    fn depends_on(&self, module: &Module) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        for c in module.conditions() {
            let mut refs = BTreeSet::new();
            c.references(&mut refs);
            for r in refs {
                match self.label_modules.get(&r) {
                    Some(ms) => out.extend(ms.iter().cloned()),
                    None => {
                        out.insert(r);
                    }
                }
            }
        }
        out
    }

    fn evaluation_order(&self) -> Result<Vec<usize>> {
        let mut order = Vec::new();
        let mut state: HashMap<String, u8> = HashMap::new();
        fn visit(
            odd: &Odd,
            name: &str,
            path: &mut Vec<String>,
            state: &mut HashMap<String, u8>,
            order: &mut Vec<usize>,
        ) -> Result<()> {
            match state.get(name) {
                Some(2) => return Ok(()),
                Some(1) => {
                    let start = path.iter().position(|p| p == name).unwrap_or(0);
                    let mut cycle: Vec<String> = path[start..].to_vec();
                    cycle.push(name.to_string());
                    return Err(Error::value(format!(
                        "ODD {}: modules refer to each other: {}",
                        odd.name,
                        cycle.join(" -> ")
                    )));
                }
                _ => {}
            }
            state.insert(name.to_string(), 1);
            let i = odd.module_index(name).expect("a known module");
            path.push(name.to_string());
            for dep in odd.depends_on(&odd.modules[i]) {
                visit(odd, &dep, path, state, order)?;
            }
            path.pop();
            state.insert(name.to_string(), 2);
            order.push(i);
            Ok(())
        }
        for m in &self.modules {
            visit(self, &m.name, &mut Vec::new(), &mut state, &mut order)?;
        }
        Ok(order)
    }

    fn items(&self) -> Items<'_> {
        self.attributes
            .iter()
            .filter_map(|a| a.item.as_ref().map(|i| (a.name.as_str(), i)))
            .collect()
    }

    /// The ODD's verdict on `values`, by attribute name (absent: missing).
    pub fn evaluate(&self, values: &Values) -> Verdict {
        let items = self.items();
        let mut truth: HashMap<String, Truth> = HashMap::new();
        for &i in &self.order {
            let module = &self.modules[i];
            let t = if module.active {
                module.evaluate(values, &truth, &items)
            } else {
                Truth::Inactive
            };
            truth.insert(module.name.clone(), t);
            if let Some(labels) = self.labels_after.get(&module.name) {
                for label in labels {
                    let verdicts: Vec<Truth> = self.label_modules[label]
                        .iter()
                        .map(|m| truth[m])
                        .filter(|t| *t != Truth::Inactive)
                        .collect();
                    let t = if verdicts.is_empty() {
                        Truth::Inactive
                    } else {
                        or(verdicts)
                    };
                    truth.insert(label.clone(), t);
                }
            }
        }
        let verdict = and(self.roots.iter().map(|r| truth[r]));
        Verdict {
            inside: verdict != Truth::False,
            assumed: verdict == Truth::Unknown,
            modules: self
                .modules
                .iter()
                .map(|m| (m.name.clone(), truth[&m.name]))
                .collect(),
        }
    }

    /// `values` with every derived attribute worked out from the others, in
    /// the attributes' order (a derived attribute may read an earlier one).
    pub fn complete(&self, values: &mut Values) {
        self.complete_except(values, None, false);
    }

    /// The derived attributes left open (or absent) worked out from the
    /// others; one unknown stays open. What the sampler judges a draw by.
    pub fn complete_open(&self, values: &mut Values) {
        let items = self.items();
        for a in &self.attributes {
            let Probe::Derived(rules) = &a.probe else {
                continue;
            };
            if values.get(&a.name).is_some_and(|v| *v != Value::Open) {
                continue;
            }
            let mut value = Value::Open;
            for (literal, condition) in rules {
                match condition.evaluate(values, &HashMap::new(), &items) {
                    Truth::Unknown => break,
                    Truth::False => continue,
                    Truth::True | Truth::Inactive => {
                        value = Value::Str(literal.clone());
                        break;
                    }
                }
            }
            values.insert(a.name.clone(), value);
        }
    }

    fn complete_except(&self, values: &mut Values, given: Option<&str>, open_if_unknown: bool) {
        let items = self.items();
        for a in &self.attributes {
            let Probe::Derived(rules) = &a.probe else {
                continue;
            };
            if Some(a.name.as_str()) == given {
                continue;
            }
            let mut value = Value::Missing;
            for (literal, condition) in rules {
                match condition.evaluate(values, &HashMap::new(), &items) {
                    Truth::Unknown => break,
                    Truth::False => continue,
                    Truth::True | Truth::Inactive => {
                        value = Value::Str(literal.clone());
                        break;
                    }
                }
            }
            if open_if_unknown && value == Value::Missing {
                value = Value::Open;
            }
            values.insert(a.name.clone(), value);
        }
    }

    fn outside_buckets_all(&self) -> IndexMap<String, Vec<String>> {
        let mut outside = IndexMap::new();
        for a in &self.attributes {
            let Some(item) = &a.item else { continue };
            let mut labels = Vec::new();
            for (i, label) in item.labels.iter().enumerate() {
                let mut values: Values = self
                    .attributes
                    .iter()
                    .map(|b| (b.name.clone(), Value::Open))
                    .collect();
                values.insert(a.name.clone(), Value::Bucket(i));
                self.complete_except(&mut values, Some(&a.name), true);
                if !self.evaluate(&values).inside {
                    labels.push(label.clone());
                }
            }
            if !labels.is_empty() {
                outside.insert(a.name.clone(), labels);
            }
        }
        outside
    }

    /// The labels of `attribute`'s buckets that lie outside the ODD.
    pub fn outside_buckets(&self, attribute: &str) -> Vec<String> {
        self.outside.get(attribute).cloned().unwrap_or_default()
    }

    pub fn attribute(&self, name: &str) -> Option<&Attribute> {
        self.attributes.iter().find(|a| a.name == name)
    }

    /// The cover items of the attributes that have buckets.
    pub fn cover_items(&self) -> Vec<&CoverItem> {
        self.attributes
            .iter()
            .filter_map(|a| a.item.as_ref())
            .collect()
    }

    /// The modules covered as situations.
    pub fn situations(&self) -> Vec<&Module> {
        self.modules.iter().filter(|m| m.situation()).collect()
    }

    /// Attributes with no buckets: monitored, but not covered.
    pub fn unmeasured(&self) -> Vec<String> {
        self.attributes
            .iter()
            .filter(|a| a.item.is_none())
            .map(|a| a.name.clone())
            .collect()
    }

    /// The ODD's definition (what a coverage file writes of it).
    pub fn definition(&self) -> Json {
        json!({
            "name": self.name,
            "text": self.text,
            "roots": self.roots,
            "modules": self.modules.iter().map(Module::describe).collect::<Vec<_>>(),
            "unmeasured": self.unmeasured(),
        })
    }

    /// A digest of what the ODD *is* once built -- its modules, every
    /// attribute's buckets and criteria, the buckets outside it -- whatever
    /// files it was read from. Two ODDs with the same structure digest judge
    /// and cover alike.
    pub fn structure_digest(&self) -> String {
        let basis = json!({
            "definition": self.definition(),
            "items": self.cover_items().iter().map(|i| i.describe()).collect::<Vec<_>>(),
            "situations": self.situations().iter().filter_map(|m| m.item.as_ref().map(|i| i.describe())).collect::<Vec<_>>(),
            "outside": self.outside,
        });
        crate::transmission::sha256_hex(crate::transmission::canonical_json(&basis).as_bytes())
    }
}

fn dups(names: &[String]) -> Vec<String> {
    let mut count: std::collections::BTreeMap<&str, usize> = Default::default();
    for n in names {
        *count.entry(n).or_default() += 1;
    }
    count
        .into_iter()
        .filter(|(_, c)| *c > 1)
        .map(|(n, _)| n.to_string())
        .collect()
}

/// A list as Python prints one: `['a', 'b']`.
pub fn py_list(items: &[String]) -> String {
    format!(
        "[{}]",
        items
            .iter()
            .map(|s| format!("'{s}'"))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attr(name: &str, buckets: Option<Buckets>) -> Attribute {
        let mut a = Attribute::new(
            name,
            Probe::External {
                kind: "probe".into(),
                spec: name.into(),
            },
            "",
            "",
            Criteria::default(),
        )
        .unwrap();
        if let Some(b) = buckets {
            a.set_buckets(b).unwrap();
        }
        a
    }

    fn leaf(a: &str, p: Predicate) -> Condition {
        Condition::Leaf {
            attribute: a.into(),
            predicate: p,
        }
    }

    fn le(x: f64) -> Predicate {
        Predicate::Interval {
            low: f64::NEG_INFINITY,
            high: x,
            include_low: true,
            include_high: true,
        }
    }

    fn lt(x: f64) -> Predicate {
        Predicate::Interval {
            low: f64::NEG_INFINITY,
            high: x,
            include_low: true,
            include_high: false,
        }
    }

    fn odd() -> Odd {
        let speed = attr("speed", Some(Buckets::Edges(vec![0.0, 30.0, 60.0, 90.0])));
        let rain = attr(
            "rain",
            Some(Buckets::Values(vec![
                Value::Str("none".into()),
                Value::Str("heavy".into()),
            ])),
        );
        let slow = Module::new(
            "slow",
            Some(Section {
                all: true,
                conditions: vec![leaf("speed", lt(60.0))],
            }),
            None,
        )
        .unwrap();
        let dry = Module::new(
            "dry",
            None,
            Some(Section {
                all: false,
                conditions: vec![leaf(
                    "rain",
                    Predicate::InSet {
                        labels: ["heavy".to_string()].into(),
                        numbers: vec![],
                    },
                )],
            }),
        )
        .unwrap();
        Odd::new("urban", vec![speed, rain], vec![slow, dry], None, "").unwrap()
    }

    #[test]
    fn missing_values_do_not_put_a_situation_out() {
        let o = odd();
        let v = o.evaluate(&Values::new());
        assert!(v.inside && v.assumed);
        let mut values = Values::new();
        values.insert("speed".into(), Value::Float(70.0));
        assert!(!o.evaluate(&values).inside);
        values.insert("speed".into(), Value::Int(50));
        values.insert("rain".into(), Value::Str("none".into()));
        let v = o.evaluate(&values);
        assert!(v.inside && !v.assumed);
    }

    #[test]
    fn buckets_the_odd_rules_out_are_outside() {
        let o = odd();
        assert_eq!(o.outside_buckets("speed"), ["[60, 90]"]);
        assert_eq!(o.outside_buckets("rain"), ["heavy"]);
    }

    #[test]
    fn a_cycle_is_refused() {
        let a = Module::new(
            "a",
            Some(Section {
                all: true,
                conditions: vec![Condition::module_holds("b", true).unwrap()],
            }),
            None,
        )
        .unwrap();
        let b = Module::new(
            "b",
            Some(Section {
                all: true,
                conditions: vec![Condition::module_holds("a", true).unwrap()],
            }),
            None,
        )
        .unwrap();
        let err = Odd::new("x", vec![], vec![a, b], None, "").unwrap_err();
        assert!(
            err.to_string()
                .contains("modules refer to each other: a -> b -> a"),
            "{err}"
        );
    }

    #[test]
    fn an_inactive_module_drops_out() {
        let mut off = Module::new(
            "off",
            Some(Section {
                all: true,
                conditions: vec![leaf("speed", le(10.0))],
            }),
            None,
        )
        .unwrap();
        off.active = false;
        let root = Module::new(
            "root",
            Some(Section {
                all: true,
                conditions: vec![Condition::module_holds("off", true).unwrap()],
            }),
            None,
        )
        .unwrap();
        let o = Odd::new("x", vec![attr("speed", None)], vec![off, root], None, "").unwrap();
        let mut values = Values::new();
        values.insert("speed".into(), Value::Float(50.0));
        assert!(o.evaluate(&values).inside);
        assert_eq!(o.roots, ["root"]);
    }
}
