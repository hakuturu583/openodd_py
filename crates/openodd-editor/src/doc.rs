//! The document the editor edits: one OpenODD YAML file as a tree of concepts
//! and a list of modules, read from YAML and written back.
//!
//! The model keeps what OpenODD gives meaning to. What it does not model --
//! `conversion`, `COD`, `OD` -- is kept as YAML text and written back as it
//! is. Comments are not kept: YAML readers drop them.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::LazyLock;

use regex::Regex;
use serde_norway::{Mapping, Value};

/// A fresh id: what the UI keys rows by, so a row keeps its inputs as others
/// come and go.
pub fn fresh_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

pub const PRIMITIVES: [&str; 4] = ["integer", "long", "float", "double"];
/// Unit types the standard's unit table knows, offered when a number is edited.
pub const UNIT_TYPES: [&str; 14] = [
    "velocity",
    "length",
    "time",
    "acceleration",
    "count",
    "angle",
    "precipitation_rate",
    "temperature",
    "luminance",
    "illuminance",
    "mass",
    "frequency",
    "bandwidth",
    "percentage",
];
static EMPTY: Vec<Concept> = Vec::new();

pub const OPERATORS: [&str; 4] = ["<", "<=", ">", ">="];

#[derive(Clone, Debug, PartialEq)]
pub struct Document {
    pub imports: Vec<String>,
    pub taxonomy: Vec<Concept>,
    pub modules: Vec<Module>,
    /// The top-level keys the editor does not model (`conversion`, `COD`, `OD`), as YAML.
    pub extra: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Concept {
    pub id: u64,
    pub name: String,
    pub kind: Kind,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    /// A record or a container of concepts.
    Group(Vec<Concept>),
    /// `<primitive> <unit type>`.
    Number {
        primitive: String,
        unit_type: String,
    },
    Boolean,
    /// A list of literals.
    Categorical(Vec<String>),
    /// Literals defined by expressions on other concepts.
    Derived(Vec<Definition>),
    /// The id of another type (a categorical defined elsewhere).
    Reference(String),
    /// Anything else (`shapefile ...`), as YAML.
    Other(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum KindTag {
    Group,
    Number,
    Boolean,
    Categorical,
    Derived,
    Reference,
    Other,
}

impl KindTag {
    pub const ALL: [KindTag; 7] = [
        KindTag::Group,
        KindTag::Number,
        KindTag::Boolean,
        KindTag::Categorical,
        KindTag::Derived,
        KindTag::Reference,
        KindTag::Other,
    ];

    pub fn label(self) -> &'static str {
        match self {
            KindTag::Group => "group",
            KindTag::Number => "number",
            KindTag::Boolean => "boolean",
            KindTag::Categorical => "categorical",
            KindTag::Derived => "derived categorical",
            KindTag::Reference => "type reference",
            KindTag::Other => "other (YAML)",
        }
    }

    pub fn from_label(label: &str) -> Option<KindTag> {
        KindTag::ALL.into_iter().find(|t| t.label() == label)
    }
}

impl Kind {
    pub fn tag(&self) -> KindTag {
        match self {
            Kind::Group(_) => KindTag::Group,
            Kind::Number { .. } => KindTag::Number,
            Kind::Boolean => KindTag::Boolean,
            Kind::Categorical(_) => KindTag::Categorical,
            Kind::Derived(_) => KindTag::Derived,
            Kind::Reference(_) => KindTag::Reference,
            Kind::Other(_) => KindTag::Other,
        }
    }

    /// This kind changed to `tag`, keeping what carries over (a categorical's
    /// literals become a derived one's, and back).
    pub fn convert(&self, tag: KindTag) -> Kind {
        if self.tag() == tag {
            return self.clone();
        }
        let literals: Vec<String> = match self {
            Kind::Categorical(l) => l.clone(),
            Kind::Derived(d) => d.iter().map(|d| d.literal.clone()).collect(),
            _ => vec![],
        };
        match tag {
            KindTag::Group => Kind::Group(vec![]),
            KindTag::Number => Kind::Number {
                primitive: "float".into(),
                unit_type: String::new(),
            },
            KindTag::Boolean => Kind::Boolean,
            KindTag::Categorical => Kind::Categorical(if literals.is_empty() {
                vec!["a".into(), "b".into()]
            } else {
                literals
            }),
            KindTag::Derived => Kind::Derived(
                if literals.is_empty() {
                    vec!["a".into(), "b".into()]
                } else {
                    literals
                }
                .into_iter()
                .map(|literal| Definition {
                    id: fresh_id(),
                    literal,
                    conditions: vec![],
                })
                .collect(),
            ),
            KindTag::Reference => Kind::Reference(String::new()),
            KindTag::Other => Kind::Other(String::new()),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Definition {
    pub id: u64,
    pub literal: String,
    pub conditions: Vec<Cond>,
}

/// `concept: expression`, or `module: true` -- one line of a section.
#[derive(Clone, Debug, PartialEq)]
pub struct Cond {
    pub id: u64,
    pub key: String,
    pub value: CondValue,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CondValue {
    /// A module or label holds (or not), or a boolean concept's value.
    Bool(bool),
    /// One of these literals.
    Literals(Vec<String>),
    /// `<= 60 km/h`, `>= heavy`.
    Compare { op: String, operand: String },
    /// `[low .. high] unit`.
    Range {
        low: String,
        high: String,
        unit: String,
    },
    /// Anything else, as YAML.
    Raw(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ValueTag {
    Bool,
    Literals,
    Compare,
    Range,
    Raw,
}

impl ValueTag {
    pub const ALL: [ValueTag; 5] = [
        ValueTag::Bool,
        ValueTag::Literals,
        ValueTag::Compare,
        ValueTag::Range,
        ValueTag::Raw,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ValueTag::Bool => "true / false",
            ValueTag::Literals => "one of",
            ValueTag::Compare => "compare",
            ValueTag::Range => "range",
            ValueTag::Raw => "YAML",
        }
    }

    pub fn from_label(label: &str) -> Option<ValueTag> {
        ValueTag::ALL.into_iter().find(|t| t.label() == label)
    }
}

impl CondValue {
    pub fn tag(&self) -> ValueTag {
        match self {
            CondValue::Bool(_) => ValueTag::Bool,
            CondValue::Literals(_) => ValueTag::Literals,
            CondValue::Compare { .. } => ValueTag::Compare,
            CondValue::Range { .. } => ValueTag::Range,
            CondValue::Raw(_) => ValueTag::Raw,
        }
    }

    pub fn empty(tag: ValueTag) -> CondValue {
        match tag {
            ValueTag::Bool => CondValue::Bool(true),
            ValueTag::Literals => CondValue::Literals(vec![]),
            ValueTag::Compare => CondValue::Compare {
                op: "<=".into(),
                operand: String::new(),
            },
            ValueTag::Range => CondValue::Range {
                low: String::new(),
                high: String::new(),
                unit: String::new(),
            },
            ValueTag::Raw => CondValue::Raw(String::new()),
        }
    }

    /// The expression as one line of text, as the YAML writes it.
    pub fn summary(&self) -> String {
        match self.to_yaml() {
            Value::String(s) => s,
            other => inline(&other),
        }
    }
}

/// `INCLUDE_AND` / `INCLUDE_OR` / `EXCLUDE_*`: its lines, and those of the one
/// nested section of the other operator.
#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    pub all: bool,
    pub conditions: Vec<Cond>,
    pub nested: Vec<Cond>,
}

impl Section {
    pub fn new(all: bool) -> Self {
        Section {
            all,
            conditions: vec![],
            nested: vec![],
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Module {
    pub id: u64,
    pub name: String,
    /// Under `ODD` (a root candidate) rather than `MODULES`.
    pub root: bool,
    pub title: String,
    pub description: String,
    pub active: bool,
    pub labels: Vec<String>,
    pub metadata: Vec<(String, String)>,
    pub include: Option<Section>,
    pub exclude: Option<Section>,
}

impl Module {
    pub fn new(name: impl Into<String>, root: bool) -> Self {
        Module {
            id: fresh_id(),
            name: name.into(),
            root,
            title: String::new(),
            description: String::new(),
            active: true,
            labels: vec![],
            metadata: vec![],
            include: Some(Section::new(true)),
            exclude: None,
        }
    }

    pub fn section_mut(&mut self, include: bool) -> &mut Option<Section> {
        if include {
            &mut self.include
        } else {
            &mut self.exclude
        }
    }

    pub fn section(&self, include: bool) -> &Option<Section> {
        if include {
            &self.include
        } else {
            &self.exclude
        }
    }
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

const MODELED: [&str; 4] = ["IMPORT", "TAXONOMY", "MODULES", "ODD"];
const MODULE_KEYS: [&str; 10] = [
    "TITLE",
    "DESCRIPTION",
    "ACTIVE",
    "LABEL",
    "LABELS",
    "METADATA",
    "INCLUDE_AND",
    "INCLUDE_OR",
    "EXCLUDE_AND",
    "EXCLUDE_OR",
];

static RANGE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\[\s*(?P<low>[^\],]+?)\s*(?:\.\.|,)\s*(?P<high>[^\]]+?)\s*\]\s*(?P<unit>\S+)?$")
        .unwrap()
});
static BOUND_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?P<op><=|>=|<|>)\s*(?P<rest>.+)$").unwrap());
static WORD_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z_][A-Za-z0-9_.\-]*$").unwrap());

/// A scalar as text (`60`, `true`, `heavy`).
pub fn text_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::Null => "null".into(),
        Value::Tagged(t) => text_of(&t.value),
        other => inline(other),
    }
}

/// A value as one line of YAML (`[a, b]`, `{x: 1}`).
pub fn inline(v: &Value) -> String {
    match v {
        Value::Sequence(items) => format!(
            "[{}]",
            items.iter().map(inline).collect::<Vec<_>>().join(", ")
        ),
        Value::Mapping(m) => format!(
            "{{{}}}",
            m.iter()
                .map(|(k, v)| format!("{}: {}", inline(k), inline(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::String(s) => {
            let yaml = serde_norway::to_string(v).unwrap_or_else(|_| s.clone());
            yaml.trim_end().to_string()
        }
        other => text_of(other),
    }
}

/// Text the user typed as a YAML value: `2` is a number, `true` a boolean,
/// `[a, b]` a list; anything that does not parse is a string.
pub fn yaml_of(text: &str) -> Value {
    let t = text.trim();
    if t.is_empty() {
        return Value::String(String::new());
    }
    match serde_norway::from_str::<Value>(t) {
        Ok(Value::Null) if t != "null" && t != "~" => Value::String(t.into()),
        Ok(v) => v,
        Err(_) => Value::String(t.into()),
    }
}

/// A literal as YAML: a number stays one (`lane_count: [1, 2, 3]`).
fn literal_yaml(text: &str) -> Value {
    match yaml_of(text) {
        v @ Value::Number(_) => v,
        _ => Value::String(text.to_string()),
    }
}

fn is_type_spec(v: &Value) -> bool {
    let Value::String(s) = v else { return false };
    match s.split_whitespace().next() {
        Some(head) => PRIMITIVES.contains(&head) || head == "boolean" || head == "shapefile",
        None => false,
    }
}

fn count_keys(v: &Value, out: &mut HashMap<String, usize>) {
    if let Value::Mapping(m) = v {
        for (k, child) in m {
            *out.entry(text_of(k)).or_default() += 1;
            count_keys(child, out);
        }
    }
}

fn tail(key: &str) -> &str {
    key.rsplit('.').next().unwrap_or(key)
}

/// Whether a mapping in the taxonomy defines literals by expressions on other
/// concepts rather than holding concepts of its own: the reader's test.
fn defines_literals(m: &Mapping, ids: &HashMap<String, usize>) -> bool {
    let mut own = HashMap::new();
    count_keys(&Value::Mapping(m.clone()), &mut own);
    m.len() >= 2
        && m.iter().all(|(_, v)| {
            let Value::Mapping(inner) = v else {
                return false;
            };
            !inner.is_empty()
                && inner
                    .iter()
                    .filter(|(k, _)| text_of(k) != "METADATA")
                    .all(|(k, _)| {
                        let t = text_of(k);
                        let t = tail(&t);
                        ids.get(t).copied().unwrap_or(0) > own.get(t).copied().unwrap_or(0)
                    })
                && inner
                    .iter()
                    .all(|(_, x)| !x.is_mapping() && !is_type_spec(x))
        })
}

fn read_concept(name: String, v: &Value, ids: &HashMap<String, usize>) -> Concept {
    let kind = match v {
        Value::Sequence(items) => Kind::Categorical(items.iter().map(text_of).collect()),
        Value::Mapping(m) if defines_literals(m, ids) => Kind::Derived(
            m.iter()
                .map(|(k, v)| Definition {
                    id: fresh_id(),
                    literal: text_of(k),
                    conditions: read_conds(v).0,
                })
                .collect(),
        ),
        Value::Mapping(m) => Kind::Group(
            m.iter()
                .map(|(k, child)| read_concept(text_of(k), child, ids))
                .collect(),
        ),
        Value::String(s) => {
            let tokens: Vec<&str> = s.split_whitespace().collect();
            let head = tokens.first().copied().unwrap_or("");
            if PRIMITIVES.contains(&head) {
                Kind::Number {
                    primitive: head.into(),
                    unit_type: tokens[1..].join(" "),
                }
            } else if head == "boolean" && tokens.len() == 1 {
                Kind::Boolean
            } else if tokens.len() == 1 && WORD_RE.is_match(head) {
                Kind::Reference(head.into())
            } else {
                Kind::Other(s.clone())
            }
        }
        other => Kind::Other(inline(other)),
    };
    Concept {
        id: fresh_id(),
        name,
        kind,
    }
}

pub fn read_value(v: &Value) -> CondValue {
    match v {
        Value::Bool(b) => CondValue::Bool(*b),
        Value::Sequence(items) => CondValue::Literals(items.iter().map(text_of).collect()),
        Value::String(s) => {
            let s = s.trim();
            if let Some(c) = RANGE_RE.captures(s) {
                CondValue::Range {
                    low: c["low"].to_string(),
                    high: c["high"].to_string(),
                    unit: c
                        .name("unit")
                        .map(|u| u.as_str().to_string())
                        .unwrap_or_default(),
                }
            } else if let Some(c) = BOUND_RE.captures(s) {
                CondValue::Compare {
                    op: c["op"].to_string(),
                    operand: c["rest"].trim().to_string(),
                }
            } else if WORD_RE.is_match(s) {
                CondValue::Literals(vec![s.to_string()])
            } else {
                CondValue::Raw(inline(v))
            }
        }
        other => CondValue::Raw(inline(other)),
    }
}

/// A section's lines, and those of the section nested in it.
fn read_conds(v: &Value) -> (Vec<Cond>, Vec<Cond>) {
    let mut out = vec![];
    let mut nested = vec![];
    if let Value::Mapping(m) = v {
        for (k, v) in m {
            let key = text_of(k);
            if key == "METADATA" {
                continue;
            }
            if key == "AND" || key == "OR" {
                nested.extend(read_conds(v).0);
                continue;
            }
            out.push(Cond {
                id: fresh_id(),
                key,
                value: read_value(v),
            });
        }
    }
    (out, nested)
}

fn read_section(m: &Mapping, include: bool) -> Result<Option<Section>, String> {
    let word = if include { "INCLUDE" } else { "EXCLUDE" };
    let and = m.get(format!("{word}_AND"));
    let or = m.get(format!("{word}_OR"));
    let (all, v) = match (and, or) {
        (Some(_), Some(_)) => return Err(format!("both {word}_AND and {word}_OR")),
        (Some(v), None) => (true, v),
        (None, Some(v)) => (false, v),
        (None, None) => return Ok(None),
    };
    let (conditions, nested) = read_conds(v);
    Ok(Some(Section {
        all,
        conditions,
        nested,
    }))
}

fn read_module(name: String, v: &Value, root: bool) -> Result<Module, String> {
    let Value::Mapping(m) = v else {
        return Err(format!("module {name}: expected a mapping"));
    };
    let unknown: Vec<String> = m
        .keys()
        .map(text_of)
        .filter(|k| !MODULE_KEYS.contains(&k.as_str()))
        .collect();
    if !unknown.is_empty() {
        return Err(format!(
            "module {name}: unknown keys {}",
            unknown.join(", ")
        ));
    }
    let get = |k: &str| m.get(k).map(text_of).unwrap_or_default();
    let mut labels = vec![];
    for key in ["LABEL", "LABELS"] {
        match m.get(key) {
            Some(Value::Sequence(items)) => labels.extend(items.iter().map(text_of)),
            Some(v) => labels.push(text_of(v)),
            None => {}
        }
    }
    let active = match m.get("ACTIVE") {
        None => true,
        Some(Value::Bool(b)) => *b,
        Some(v) => !text_of(v).eq_ignore_ascii_case("false"),
    };
    let metadata = match m.get("METADATA") {
        Some(Value::Mapping(meta)) => meta.iter().map(|(k, v)| (text_of(k), text_of(v))).collect(),
        _ => vec![],
    };
    let at = |e: String| format!("module {name}: {e}");
    Ok(Module {
        id: fresh_id(),
        title: get("TITLE"),
        description: get("DESCRIPTION"),
        active,
        labels,
        metadata,
        include: read_section(m, true).map_err(at)?,
        exclude: read_section(m, false).map_err(at)?,
        name,
        root,
    })
}

impl Document {
    pub fn empty() -> Self {
        Document {
            imports: vec![],
            taxonomy: vec![],
            modules: vec![],
            extra: String::new(),
        }
    }

    /// Read an OpenODD document.
    pub fn from_yaml(text: &str) -> Result<Self, String> {
        let mut docs = vec![];
        for d in serde_norway::Deserializer::from_str(text) {
            let v: Value =
                serde::Deserialize::deserialize(d).map_err(|e| format!("not YAML: {e}"))?;
            if !v.is_null() {
                docs.push(v);
            }
        }
        if docs.len() > 1 {
            return Err("the editor reads one YAML document per file (no `---`)".into());
        }
        let root = match docs.pop() {
            None => return Ok(Document::empty()),
            Some(Value::Mapping(m)) => m,
            Some(_) => return Err("an OpenODD document must be a mapping".into()),
        };
        if root.contains_key("openodd") && root.contains_key("probes") {
            return Err(
                "this is a binding file: open the OpenODD file it names under `openodd`".into(),
            );
        }
        let imports = match root.get("IMPORT") {
            Some(Value::Sequence(items)) => items.iter().map(text_of).collect(),
            Some(Value::Null) | None => vec![],
            Some(v) => vec![text_of(v)],
        };
        let taxonomy_node = root.get("TAXONOMY").cloned().unwrap_or(Value::Null);
        let mut ids = HashMap::new();
        count_keys(&taxonomy_node, &mut ids);
        let taxonomy = match &taxonomy_node {
            Value::Mapping(m) => m
                .iter()
                .map(|(k, v)| read_concept(text_of(k), v, &ids))
                .collect(),
            Value::Null => vec![],
            _ => return Err("TAXONOMY: expected a mapping".into()),
        };
        let mut modules = vec![];
        for (section, root_flag) in [("MODULES", false), ("ODD", true)] {
            match root.get(section) {
                Some(Value::Mapping(m)) => {
                    for (k, v) in m {
                        modules.push(read_module(text_of(k), v, root_flag)?);
                    }
                }
                Some(Value::Null) | None => {}
                Some(_) => return Err(format!("{section}: expected a mapping of modules")),
            }
        }
        let mut extra = Mapping::new();
        for (k, v) in &root {
            if !MODELED.contains(&text_of(k).as_str()) {
                extra.insert(k.clone(), v.clone());
            }
        }
        let extra = if extra.is_empty() {
            String::new()
        } else {
            serde_norway::to_string(&extra).map_err(|e| e.to_string())?
        };
        Ok(Document {
            imports,
            taxonomy,
            modules,
            extra,
        })
    }

    // -----------------------------------------------------------------------
    // Writing
    // -----------------------------------------------------------------------

    pub fn to_value(&self) -> Result<Value, String> {
        let mut root = Mapping::new();
        if !self.imports.is_empty() {
            root.insert(
                "IMPORT".into(),
                Value::Sequence(
                    self.imports
                        .iter()
                        .map(|s| Value::String(s.clone()))
                        .collect(),
                ),
            );
        }
        if !self.taxonomy.is_empty() {
            root.insert("TAXONOMY".into(), concepts_yaml(&self.taxonomy));
        }
        for (section, root_flag) in [("MODULES", false), ("ODD", true)] {
            let mut m = Mapping::new();
            for module in self.modules.iter().filter(|m| m.root == root_flag) {
                m.insert(Value::String(module.name.clone()), module.to_yaml());
            }
            if !m.is_empty() {
                root.insert(section.into(), Value::Mapping(m));
            }
        }
        if !self.extra.trim().is_empty() {
            match serde_norway::from_str::<Value>(&self.extra) {
                Ok(Value::Mapping(extra)) => {
                    for (k, v) in extra {
                        if MODELED.contains(&text_of(&k).as_str()) {
                            return Err(format!(
                                "other sections: {} is edited in its own tab",
                                text_of(&k)
                            ));
                        }
                        root.insert(k, v);
                    }
                }
                Ok(Value::Null) => {}
                Ok(_) => return Err("other sections: expected a mapping".into()),
                Err(e) => return Err(format!("other sections: not YAML: {e}")),
            }
        }
        Ok(Value::Mapping(root))
    }

    pub fn to_yaml(&self) -> Result<String, String> {
        let v = self.to_value()?;
        if v.as_mapping().is_some_and(Mapping::is_empty) {
            return Ok(String::new());
        }
        serde_norway::to_string(&v).map_err(|e| e.to_string())
    }

    // -----------------------------------------------------------------------
    // What the editor offers
    // -----------------------------------------------------------------------

    /// Every leaf concept, by its dotted name, with its kind.
    pub fn leaves(&self) -> Vec<(String, Kind)> {
        fn walk(cs: &[Concept], prefix: &str, out: &mut Vec<(String, Kind)>) {
            for c in cs {
                let name = if prefix.is_empty() {
                    c.name.clone()
                } else {
                    format!("{prefix}.{}", c.name)
                };
                match &c.kind {
                    Kind::Group(children) => walk(children, &name, out),
                    k => out.push((name, k.clone())),
                }
            }
        }
        let mut out = vec![];
        walk(&self.taxonomy, "", &mut out);
        out
    }

    /// The labels the modules carry.
    pub fn labels(&self) -> Vec<String> {
        let mut out: Vec<String> = vec![];
        for m in &self.modules {
            for l in &m.labels {
                if !out.contains(l) {
                    out.push(l.clone());
                }
            }
        }
        out
    }

    /// What a key of a section names: a concept's kind, or a module / label.
    pub fn key_kind(&self, key: &str) -> KeyKind {
        if self.modules.iter().any(|m| m.name == key) {
            return KeyKind::Module;
        }
        if self.labels().iter().any(|l| l == key) {
            return KeyKind::Label;
        }
        let leaves = self.leaves();
        let found = leaves
            .iter()
            .find(|(n, _)| n == key)
            .or_else(|| leaves.iter().find(|(n, _)| tail(n) == key));
        match found {
            Some((_, kind)) => KeyKind::Concept(kind.clone()),
            None => KeyKind::Unknown,
        }
    }

    /// A concept by its id, anywhere in the tree.
    pub fn concept_mut(&mut self, id: u64) -> Option<&mut Concept> {
        fn find(cs: &mut [Concept], id: u64) -> Option<&mut Concept> {
            for c in cs {
                if c.id == id {
                    return Some(c);
                }
                if let Kind::Group(children) = &mut c.kind {
                    if let Some(found) = find(children, id) {
                        return Some(found);
                    }
                }
            }
            None
        }
        find(&mut self.taxonomy, id)
    }

    pub fn concept(&self, id: u64) -> Option<&Concept> {
        fn find(cs: &[Concept], id: u64) -> Option<&Concept> {
            for c in cs {
                if c.id == id {
                    return Some(c);
                }
                if let Kind::Group(children) = &c.kind {
                    if let Some(found) = find(children, id) {
                        return Some(found);
                    }
                }
            }
            None
        }
        find(&self.taxonomy, id)
    }

    /// The id of the concept a dotted name names.
    pub fn concept_id(&self, path: &str) -> Option<u64> {
        let mut list = &self.taxonomy;
        let mut found = None;
        for part in path.split('.') {
            let c = list.iter().find(|c| c.name == part)?;
            found = Some(c.id);
            list = match &c.kind {
                Kind::Group(children) => children,
                _ => &EMPTY,
            };
        }
        found
    }

    /// The dotted name of a concept.
    pub fn concept_path(&self, id: u64) -> Option<String> {
        fn find(cs: &[Concept], id: u64, prefix: &mut Vec<String>) -> bool {
            for c in cs {
                prefix.push(c.name.clone());
                if c.id == id {
                    return true;
                }
                if let Kind::Group(children) = &c.kind {
                    if find(children, id, prefix) {
                        return true;
                    }
                }
                prefix.pop();
            }
            false
        }
        let mut path = vec![];
        find(&self.taxonomy, id, &mut path).then(|| path.join("."))
    }

    /// The list a concept is in (the taxonomy, or its group's), for adding,
    /// moving and removing it. `None` for the taxonomy itself.
    pub fn siblings_mut(&mut self, parent: Option<u64>) -> Option<&mut Vec<Concept>> {
        match parent {
            None => Some(&mut self.taxonomy),
            Some(id) => match &mut self.concept_mut(id)?.kind {
                Kind::Group(children) => Some(children),
                _ => None,
            },
        }
    }

    /// The group a concept is in (`Some(None)`: the taxonomy).
    pub fn parent_of(&self, id: u64) -> Option<Option<u64>> {
        fn find(cs: &[Concept], id: u64, parent: Option<u64>) -> Option<Option<u64>> {
            for c in cs {
                if c.id == id {
                    return Some(parent);
                }
                if let Kind::Group(children) = &c.kind {
                    if let Some(p) = find(children, id, Some(c.id)) {
                        return Some(p);
                    }
                }
            }
            None
        }
        find(&self.taxonomy, id, None)
    }

    pub fn remove_concept(&mut self, id: u64) {
        if let Some(parent) = self.parent_of(id) {
            if let Some(list) = self.siblings_mut(parent) {
                list.retain(|c| c.id != id);
            }
        }
    }

    /// Move a concept one place up (`-1`) or down (`1`) among its siblings.
    pub fn move_concept(&mut self, id: u64, by: isize) {
        if let Some(parent) = self.parent_of(id) {
            if let Some(list) = self.siblings_mut(parent) {
                move_in(list, |c| c.id == id, by);
            }
        }
    }

    pub fn module_mut(&mut self, id: u64) -> Option<&mut Module> {
        self.modules.iter_mut().find(|m| m.id == id)
    }

    pub fn module(&self, id: u64) -> Option<&Module> {
        self.modules.iter().find(|m| m.id == id)
    }

    /// The lines a condition list holds: a module's section (or its nested
    /// one), or a derived literal's definition.
    pub fn conds_mut(&mut self, at: CondsAt) -> Option<&mut Vec<Cond>> {
        match at {
            CondsAt::Section {
                module,
                include,
                nested,
            } => {
                let s = self.module_mut(module)?.section_mut(include).as_mut()?;
                Some(if nested {
                    &mut s.nested
                } else {
                    &mut s.conditions
                })
            }
            CondsAt::Definition { concept, literal } => {
                match &mut self.concept_mut(concept)?.kind {
                    Kind::Derived(defs) => defs
                        .iter_mut()
                        .find(|d| d.id == literal)
                        .map(|d| &mut d.conditions),
                    _ => None,
                }
            }
        }
    }

    pub fn conds(&self, at: CondsAt) -> Option<&Vec<Cond>> {
        match at {
            CondsAt::Section {
                module,
                include,
                nested,
            } => {
                let s = self.module(module)?.section(include).as_ref()?;
                Some(if nested { &s.nested } else { &s.conditions })
            }
            CondsAt::Definition { concept, literal } => match &self.concept(concept)?.kind {
                Kind::Derived(defs) => defs.iter().find(|d| d.id == literal).map(|d| &d.conditions),
                _ => None,
            },
        }
    }
}

/// Where a list of conditions lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CondsAt {
    Section {
        module: u64,
        include: bool,
        nested: bool,
    },
    Definition {
        concept: u64,
        literal: u64,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum KeyKind {
    Concept(Kind),
    Module,
    Label,
    Unknown,
}

pub fn move_in<T>(list: &mut [T], is: impl Fn(&T) -> bool, by: isize) {
    if let Some(i) = list.iter().position(is) {
        let j = i as isize + by;
        if j >= 0 && (j as usize) < list.len() {
            list.swap(i, j as usize);
        }
    }
}

fn concepts_yaml(cs: &[Concept]) -> Value {
    let mut m = Mapping::new();
    for c in cs {
        m.insert(Value::String(c.name.clone()), c.kind.to_yaml());
    }
    Value::Mapping(m)
}

impl Kind {
    pub fn to_yaml(&self) -> Value {
        match self {
            Kind::Group(children) => concepts_yaml(children),
            Kind::Number {
                primitive,
                unit_type,
            } => Value::String(format!("{primitive} {unit_type}").trim().to_string()),
            Kind::Boolean => Value::String("boolean".into()),
            Kind::Categorical(literals) => {
                Value::Sequence(literals.iter().map(|l| literal_yaml(l)).collect())
            }
            Kind::Derived(defs) => {
                let mut m = Mapping::new();
                for d in defs {
                    m.insert(literal_yaml(&d.literal), conds_yaml(&d.conditions, &[], ""));
                }
                Value::Mapping(m)
            }
            Kind::Reference(id) => Value::String(id.clone()),
            Kind::Other(text) => yaml_of(text),
        }
    }
}

impl CondValue {
    pub fn to_yaml(&self) -> Value {
        match self {
            CondValue::Bool(b) => Value::Bool(*b),
            CondValue::Literals(l) if l.len() == 1 => literal_yaml(&l[0]),
            CondValue::Literals(l) => Value::Sequence(l.iter().map(|x| literal_yaml(x)).collect()),
            CondValue::Compare { op, operand } => Value::String(format!("{op} {}", operand.trim())),
            CondValue::Range { low, high, unit } => Value::String(
                format!("[{} .. {}] {}", low.trim(), high.trim(), unit.trim())
                    .trim()
                    .to_string(),
            ),
            CondValue::Raw(text) => yaml_of(text),
        }
    }
}

fn conds_yaml(conds: &[Cond], nested: &[Cond], nested_key: &str) -> Value {
    let mut m = Mapping::new();
    for c in conds {
        m.insert(Value::String(c.key.clone()), c.value.to_yaml());
    }
    if !nested.is_empty() {
        m.insert(nested_key.into(), conds_yaml(nested, &[], ""));
    }
    Value::Mapping(m)
}

impl Module {
    pub fn to_yaml(&self) -> Value {
        let mut m = Mapping::new();
        let text = |s: &str| Value::String(s.to_string());
        if !self.title.is_empty() {
            m.insert("TITLE".into(), text(&self.title));
        }
        if !self.description.is_empty() {
            m.insert("DESCRIPTION".into(), text(&self.description));
        }
        if !self.active {
            m.insert("ACTIVE".into(), Value::Bool(false));
        }
        match self.labels.as_slice() {
            [] => {}
            [one] => {
                m.insert("LABEL".into(), text(one));
            }
            many => {
                m.insert(
                    "LABELS".into(),
                    Value::Sequence(many.iter().map(|l| text(l)).collect()),
                );
            }
        }
        if !self.metadata.is_empty() {
            let mut meta = Mapping::new();
            for (k, v) in &self.metadata {
                meta.insert(text(k), text(v));
            }
            m.insert("METADATA".into(), Value::Mapping(meta));
        }
        for (word, section) in [("INCLUDE", &self.include), ("EXCLUDE", &self.exclude)] {
            if let Some(s) = section {
                let (op, other) = if s.all { ("AND", "OR") } else { ("OR", "AND") };
                m.insert(
                    Value::String(format!("{word}_{op}")),
                    conds_yaml(&s.conditions, &s.nested, other),
                );
            }
        }
        Value::Mapping(m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const URBAN: &str = include_str!("../examples/urban.yaml");
    const DEFAULT: &str = include_str!("../../../python/openodd/odds/default/default.yaml");

    fn same(a: &str, b: &str) {
        let a: Value = serde_norway::from_str(a).unwrap();
        let b: Value = serde_norway::from_str(b).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn the_examples_read_and_write_back_the_same() {
        for text in [URBAN, DEFAULT] {
            let doc = Document::from_yaml(text).unwrap();
            same(&doc.to_yaml().unwrap(), text);
        }
    }

    #[test]
    fn a_derived_categorical_is_told_from_a_record() {
        let doc = Document::from_yaml(URBAN).unwrap();
        let leaves = doc.leaves();
        let kind = |n: &str| leaves.iter().find(|(x, _)| x == n).unwrap().1.tag();
        assert_eq!(kind("environment.rainfall"), KindTag::Derived);
        assert_eq!(kind("environment.rainfall_rate"), KindTag::Number);
        assert_eq!(kind("scenery.road_type"), KindTag::Categorical);
        let record = "TAXONOMY:\n  surface:\n    a: {x: [1, 2]}\n    b: {y: [3]}\n";
        let doc = Document::from_yaml(record).unwrap();
        assert_eq!(doc.taxonomy[0].kind.tag(), KindTag::Group);
    }

    #[test]
    fn expressions_read_into_their_forms() {
        let v = |s: &str| read_value(&serde_norway::from_str(s).unwrap());
        assert_eq!(
            v("'<= 60 km/h'"),
            CondValue::Compare {
                op: "<=".into(),
                operand: "60 km/h".into()
            }
        );
        assert_eq!(
            v("'[0 .. 40] km/h'"),
            CondValue::Range {
                low: "0".into(),
                high: "40".into(),
                unit: "km/h".into()
            }
        );
        assert_eq!(v("heavy"), CondValue::Literals(vec!["heavy".into()]));
        assert_eq!(
            v("[a, b]"),
            CondValue::Literals(vec!["a".into(), "b".into()])
        );
        assert_eq!(v("false"), CondValue::Bool(false));
        assert_eq!(v("3"), CondValue::Raw("3".into()));
        assert_eq!(CondValue::Raw("3".into()).to_yaml(), yaml_of("3"));
    }

    #[test]
    fn a_nested_section_is_kept() {
        let doc = Document::from_yaml(URBAN).unwrap();
        let town = doc.modules.iter().find(|m| m.name == "town_roads").unwrap();
        let s = town.include.as_ref().unwrap();
        assert!(s.all);
        assert_eq!(s.nested.len(), 2);
        assert!(!doc.modules.iter().find(|m| m.name == "dark").unwrap().root);
        assert!(
            doc.modules
                .iter()
                .find(|m| m.name == "urban_shuttle")
                .unwrap()
                .root
        );
    }

    #[test]
    fn other_sections_are_kept_as_yaml() {
        let text = "TAXONOMY:\n  d: float length\nconversion:\n  length:\n    ft:\n      m: {scale: 0.3048}\n";
        let doc = Document::from_yaml(text).unwrap();
        assert!(doc.extra.contains("conversion"));
        same(&doc.to_yaml().unwrap(), text);
    }

    #[test]
    fn a_binding_file_is_refused() {
        let err = Document::from_yaml("openodd: [x.yaml]\nprobes: {}\n").unwrap_err();
        assert!(err.contains("binding"), "{err}");
    }

    #[test]
    fn kinds_convert_keeping_literals() {
        let k = Kind::Categorical(vec!["x".into(), "y".into()]);
        let d = k.convert(KindTag::Derived);
        let Kind::Derived(defs) = &d else { panic!() };
        assert_eq!(defs.len(), 2);
        assert_eq!(d.convert(KindTag::Categorical), k);
    }

    #[test]
    fn concepts_move_and_go() {
        let mut doc = Document::from_yaml(URBAN).unwrap();
        let env = doc.taxonomy[1].id;
        let first = match &doc.taxonomy[1].kind {
            Kind::Group(c) => c[0].id,
            _ => panic!(),
        };
        assert_eq!(doc.concept_path(first).unwrap(), "environment.illumination");
        doc.move_concept(first, 1);
        assert_eq!(doc.parent_of(first), Some(Some(env)));
        let Kind::Group(c) = &doc.concept(env).unwrap().kind else {
            panic!()
        };
        assert_eq!(c[1].id, first);
        doc.remove_concept(first);
        assert!(doc.concept(first).is_none());
    }
}
