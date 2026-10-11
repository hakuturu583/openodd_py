//! Read an ODD written in ASAM OpenODD 1.0 YAML (chapter 10 of the standard).
//!
//! An OpenODD document holds what an ODD *is*. How an application *measures*
//! it lives in a separate **binding file**: which probe reads each concept, and
//! its buckets. The standard allows no constructs of its own in an OpenODD
//! document, so they are kept apart. Probes are the backend's: the reader only
//! records which one a concept names, and asks the backend ([`Resolver`]) the
//! unit it reads in.
//!
//! What is read:
//!
//! * `IMPORT`: other OpenODD files, relative to the importing one, or else in
//!   the directories of the other sources (a cycle is refused). Sources can be
//!   files in git repositories at a revision ([`crate::sources`]).
//! * `TAXONOMY`: the concepts. Nested mappings are records and containers. A
//!   leaf is `<primitive> <unit type>` (a number), `boolean`, a list (a
//!   categorical), a mapping of literals to expressions on other concepts (a
//!   categorical defined by them), or the id of a categorical.
//! * `MODULES` and `ODD`: the modules; those under `ODD` are the root
//!   candidates. A module has `TITLE`, `DESCRIPTION`, `ACTIVE`, `LABEL(S)`,
//!   `METADATA`, at most one `INCLUDE_AND`/`INCLUDE_OR` and at most one
//!   `EXCLUDE_AND`/`EXCLUDE_OR`; a section may nest one level of the other
//!   operator.
//! * `conversion`: units the document adds.
//!
//! Not read: `COD` / `OD` records, numeric terms (`1.75*ego_width`), `$`
//! parameters and condition-level metadata.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use indexmap::IndexMap;
use regex::Regex;

use crate::error::{Error, Result};
use crate::items::{Buckets, Criteria};
use crate::model::{Attribute, Condition, Module, Odd, Predicate, Probe, Section};
use crate::sources::{self, GitSource};
use crate::transmission::{Origin, Transmission};
use crate::units::{normalize_unit, Conversion, Units};
use crate::value::Value;
use crate::yaml::{empty_map, parse_all, Mapping, Node, NodeExt};

/// What a backend says of the probes a binding file names.
pub trait Resolver {
    /// Binding keys naming one of the backend's readers (`measure`, ...).
    fn reader_keys(&self) -> Vec<String> {
        Vec::new()
    }
    /// The unit a `probe:` reads in (`""`: none or unknown); `Err` if there is no such probe.
    fn probe_unit(&self, spec: &str) -> std::result::Result<String, String>;
    /// The unit a reader reads `value` in; `Err` if it cannot.
    fn reader_unit(&self, key: &str, value: &str) -> std::result::Result<String, String> {
        let _ = value;
        Err(format!("no {key:?} readers"))
    }
}

/// A resolver that knows no probe's unit and accepts every name: for reading
/// an ODD's structure without measuring it (a hub, a linter).
pub struct AnyProbe;

impl Resolver for AnyProbe {
    fn reader_keys(&self) -> Vec<String> {
        vec!["measure".into()]
    }
    fn probe_unit(&self, _spec: &str) -> std::result::Result<String, String> {
        Ok(String::new())
    }
    fn reader_unit(&self, _key: &str, _value: &str) -> std::result::Result<String, String> {
        Ok(String::new())
    }
}

/// One source of an ODD.
#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    Path(PathBuf),
    /// YAML text.
    Text(String),
    Git(GitSource),
}

/// How a run reads an ODD: options besides its sources.
#[derive(Default)]
pub struct Options {
    /// Concept -> how it is measured (the `probes` of a binding file).
    pub bindings: Option<Node>,
    pub name: Option<String>,
    pub text: String,
    /// Module -> criteria: the modules covered as situations.
    pub situations: Option<Node>,
}

/// An ODD read from files, with the transmission it was read from.
#[derive(Debug, Clone)]
pub struct Loaded {
    pub odd: Odd,
    pub transmission: Transmission,
    /// The git sources read, with the commits their revisions named.
    pub sources: Vec<Origin>,
    /// The ODD's declared version (its root modules' `METADATA: {version}`).
    pub version: Option<String>,
}

impl Loaded {
    pub fn revision(&self) -> String {
        self.transmission.revision()
    }
}

// ---------------------------------------------------------------------------
// Files: on disk, or in a transmission
// ---------------------------------------------------------------------------

/// Where the reader takes files from.
enum Files<'a> {
    Disk,
    /// A transmission's files, under a virtual directory.
    Memory(&'a Transmission),
}

const MEMORY_ROOT: &str = "/openodd-transmission";

impl Files<'_> {
    fn read(&self, path: &Path) -> Result<Vec<u8>> {
        match self {
            Files::Disk => std::fs::read(path)
                .map_err(|e| Error::open_odd(format!("cannot read {}: {e}", path.display()))),
            Files::Memory(t) => {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                t.file(&name).map(|f| f.data.clone()).ok_or_else(|| {
                    Error::open_odd(format!("cannot read {name}: not in the transmission"))
                })
            }
        }
    }

    fn is_file(&self, path: &Path) -> bool {
        match self {
            Files::Disk => path.is_file(),
            Files::Memory(t) => {
                path.parent() == Some(Path::new(MEMORY_ROOT))
                    && path
                        .file_name()
                        .is_some_and(|n| t.file(&n.to_string_lossy()).is_some())
            }
        }
    }

    fn resolve(&self, path: &Path) -> PathBuf {
        match self {
            Files::Disk => path.canonicalize().unwrap_or_else(|_| absolute(path)),
            Files::Memory(_) => path.to_path_buf(),
        }
    }
}

fn absolute(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|d| d.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    }
}

fn stem(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Documents
// ---------------------------------------------------------------------------

const KNOWN_KEYS: [&str; 7] = [
    "IMPORT",
    "TAXONOMY",
    "MODULES",
    "ODD",
    "COD",
    "OD",
    "conversion",
];

struct Documents<'f> {
    files: &'f Files<'f>,
    taxonomy: Mapping,
    modules: IndexMap<String, Node>,
    roots: Vec<String>,
    conversion: Mapping,
    first_stem: Option<String>,
    seen: BTreeSet<PathBuf>,
    names: HashMap<String, PathBuf>,
    origins: HashMap<String, String>,
    search: Vec<PathBuf>,
    provenance: Vec<Origin>,
    checkouts: Vec<(PathBuf, Origin)>,
    transmission: Transmission,
    texts: usize,
}

impl<'f> Documents<'f> {
    fn new(files: &'f Files<'f>) -> Self {
        Documents {
            files,
            taxonomy: Mapping::new(),
            modules: IndexMap::new(),
            roots: Vec::new(),
            conversion: Mapping::new(),
            first_stem: None,
            seen: BTreeSet::new(),
            names: HashMap::new(),
            origins: HashMap::new(),
            search: Vec::new(),
            provenance: Vec::new(),
            checkouts: Vec::new(),
            transmission: Transmission::default(),
            texts: 0,
        }
    }

    fn origin_of(&self, path: &Path) -> Option<Origin> {
        for (root, origin) in &self.checkouts {
            if let Ok(rel) = path.strip_prefix(root) {
                return Some(Origin {
                    path: rel.to_string_lossy().replace('\\', "/"),
                    ..origin.clone()
                });
            }
        }
        None
    }

    fn record_file(&mut self, path: &Path, data: &[u8]) -> Result<()> {
        let origin = self.origin_of(path);
        self.transmission
            .add(&file_name(path), data.to_vec(), origin)
    }

    /// The file an `IMPORT` of `name` in a file under `base` names.
    fn locate(&self, base: &Path, name: &str) -> Result<PathBuf> {
        let mut here = base.join(name);
        if !self.files.is_file(&here) {
            let found: BTreeSet<PathBuf> = self
                .search
                .iter()
                .map(|root| root.join(name))
                .filter(|p| self.files.is_file(p))
                .map(|p| self.files.resolve(&p))
                .collect();
            if found.len() > 1 {
                return Err(Error::open_odd(format!(
                    "IMPORT {name} is in several sources: {}",
                    found
                        .iter()
                        .map(|p| p.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                )));
            }
            if let Some(f) = found.into_iter().next() {
                here = f;
            }
        }
        let roots: Vec<&PathBuf> = self.checkouts.iter().map(|(r, _)| r).collect();
        let under = |p: &Path, roots: &[&PathBuf]| {
            let p = self.files.resolve(p);
            roots.iter().any(|r| p.starts_with(r))
        };
        let search: Vec<&PathBuf> = self.search.iter().collect();
        if under(base, &roots) && !under(&here, &search) {
            return Err(Error::open_odd(format!(
                "IMPORT {name} in {}: a file from git imports only from the sources",
                base.display()
            )));
        }
        Ok(here)
    }

    fn read(&mut self, source: ReadSource, stack: &mut Vec<PathBuf>) -> Result<()> {
        let (path, base, loaded) = match source {
            ReadSource::Path(p, loaded) => {
                let path = self.files.resolve(&p);
                if stack.contains(&path) {
                    let mut chain: Vec<String> = stack.iter().map(|p| file_name(p)).collect();
                    chain.push(file_name(&path));
                    return Err(Error::open_odd(format!(
                        "IMPORT cycle: {}",
                        chain.join(" -> ")
                    )));
                }
                if self.seen.contains(&path) {
                    return Ok(());
                }
                if let Some(other) = self.names.get(&file_name(&path)) {
                    return Err(Error::open_odd(format!(
                        "two files named {} ({} and {}): OpenODD makes file names unique within a transmission",
                        file_name(&path),
                        other.display(),
                        path.display()
                    )));
                }
                let data = self.files.read(&path)?;
                self.seen.insert(path.clone());
                self.names.insert(file_name(&path), path.clone());
                self.record_file(&path, &data)?;
                if self.first_stem.is_none() {
                    self.first_stem = Some(stem(&path));
                }
                let loaded = match loaded {
                    Some(l) => l,
                    None => parse_all(&String::from_utf8_lossy(&data))?,
                };
                let base = path.parent().map(Path::to_path_buf).unwrap_or_default();
                (Some(path), base, loaded)
            }
            ReadSource::Text(text) => {
                self.texts += 1;
                self.transmission.add(
                    &format!("text-{}.yaml", self.texts),
                    text.as_bytes().to_vec(),
                    None,
                )?;
                let base = std::env::current_dir().unwrap_or_default();
                (None, base, parse_all(&text)?)
            }
        };
        if let Some(p) = &path {
            stack.push(p.clone());
        }
        let result = self.read_docs(path.as_deref(), &base, loaded, stack);
        if path.is_some() {
            stack.pop();
        }
        result
    }

    fn read_docs(
        &mut self,
        path: Option<&Path>,
        base: &Path,
        loaded: Vec<Node>,
        stack: &mut Vec<PathBuf>,
    ) -> Result<()> {
        for doc in loaded {
            let Some(map) = doc.as_mapping() else {
                return Err(Error::open_odd("an OpenODD document must be a mapping"));
            };
            let mut unknown: Vec<String> = map
                .iter()
                .map(|(k, _)| k.py_str())
                .filter(|k| !KNOWN_KEYS.contains(&k.as_str()))
                .collect();
            unknown.sort();
            if !unknown.is_empty() {
                let hint = if unknown.iter().any(|k| k == "COVERAGE") {
                    "; probes and buckets belong in a binding file (load_odd_binding)"
                } else {
                    ""
                };
                return Err(Error::open_odd(format!(
                    "not OpenODD YAML keys: {}{hint}",
                    crate::model::py_list(&unknown)
                )));
            }
            for imported in doc.get("IMPORT").map(NodeExt::as_list).unwrap_or_default() {
                let target = self.locate(base, &imported.py_str())?;
                self.read(ReadSource::Path(target, None), stack)?;
            }
            let origin = path.map(file_name).unwrap_or_else(|| "<text>".into());
            let taxonomy = doc
                .get("TAXONOMY")
                .filter(|n| !n.is_falsy())
                .cloned()
                .unwrap_or(empty_map());
            let mut into = std::mem::take(&mut self.taxonomy);
            merge(
                &mut into,
                &taxonomy,
                "TAXONOMY",
                &origin,
                Some(&mut self.origins),
            )?;
            self.taxonomy = into;
            let conversion = doc
                .get("conversion")
                .filter(|n| !n.is_falsy())
                .cloned()
                .unwrap_or(empty_map());
            let mut into = std::mem::take(&mut self.conversion);
            merge(&mut into, &conversion, "conversion", &origin, None)?;
            self.conversion = into;
            for section in ["MODULES", "ODD"] {
                let Some(modules) = doc.get(section).filter(|n| !n.is_falsy()) else {
                    continue;
                };
                let Some(pairs) = modules.as_mapping() else {
                    return Err(Error::open_odd(format!(
                        "{section}: expected a mapping of modules"
                    )));
                };
                for (name, mdef) in pairs {
                    let name = name.py_str();
                    if self.modules.contains_key(&name) {
                        return Err(Error::open_odd(format!("module {name} is defined twice")));
                    }
                    self.modules.insert(name.clone(), mdef.clone());
                    if section == "ODD" {
                        self.roots.push(name);
                    }
                }
            }
        }
        Ok(())
    }
}

enum ReadSource {
    Path(PathBuf, Option<Vec<Node>>),
    Text(String),
}

/// Merge `doc` into `into`. Containers merge; with `origins`, a concept
/// defined in two files is refused even when both say the same.
fn merge(
    into: &mut Mapping,
    doc: &Node,
    where_: &str,
    origin: &str,
    mut origins: Option<&mut HashMap<String, String>>,
) -> Result<()> {
    let Some(pairs) = doc.as_mapping() else {
        return Err(Error::open_odd(format!("{where_}: expected a mapping")));
    };
    for (key, value) in pairs {
        let name = format!("{where_}.{}", key.py_str());
        match into.get_mut(key) {
            Some(Node::Mapping(inner)) if value.is_mapping() => {
                merge(inner, value, &name, origin, origins.as_deref_mut())?;
            }
            Some(existing) if origins.is_some() || existing != value => {
                let first = match &origins {
                    Some(o) => format!(
                        " (in {} and {origin})",
                        o.get(&name).cloned().unwrap_or_default()
                    ),
                    None => String::new(),
                };
                let same = if origins.is_some() {
                    ""
                } else {
                    ", differently"
                };
                return Err(Error::open_odd(format!(
                    "{name} is defined twice{same}{first}"
                )));
            }
            Some(_) => {}
            None => {
                into.insert(key.clone(), value.clone());
                if let Some(o) = origins.as_deref_mut() {
                    record(o, &name, value, origin);
                }
            }
        }
    }
    Ok(())
}

fn record(origins: &mut HashMap<String, String>, name: &str, value: &Node, origin: &str) {
    origins.insert(name.to_string(), origin.to_string());
    if let Some(pairs) = value.as_mapping() {
        for (k, child) in pairs {
            record(origins, &format!("{name}.{}", k.py_str()), child, origin);
        }
    }
}

// ---------------------------------------------------------------------------
// Taxonomy
// ---------------------------------------------------------------------------

const BINDING_KEYS: [&str; 10] = [
    "probe", "unit", "values", "buckets", "range", "every", "text", "target", "cover_by",
    "min_stay",
];
const PRIMITIVES: [&str; 4] = ["integer", "long", "float", "double"];
/// The standard's keyword for a missing value, and its accepted replacements.
const UNKNOWN: [&str; 4] = ["unknown", "none", "null", "undefined"];
const NUMBER: &str = r"[+-]?(?:\d+(?:\.\d*)?|\.\d+)(?:[eE][+-]?\d+)?";

static IDENTIFIER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z_][A-Za-z0-9_.\-]*$").unwrap());
static RANGE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\[\s*(?P<low>[^\],]+?)\s*(?:\.\.|,)\s*(?P<high>[^\]]+?)\s*\]\s*(?P<unit>\S+)?$")
        .unwrap()
});
static BOUND_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?P<op><=|>=|<|>)\s*(?P<rest>.+)$").unwrap());
static QUANTITY_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r"^(?P<value>{NUMBER})\s*(?P<unit>\S+)?$")).unwrap());

#[derive(Debug, Clone, PartialEq)]
enum Kind {
    Number,
    Boolean,
    Categorical,
    Reference,
}

#[derive(Debug, Clone)]
struct Concept {
    path: Vec<String>,
    kind: Kind,
    unit_type: String,
    literals: Vec<String>,
    /// literal -> the expressions defining it (a derived categorical)
    definitions: Vec<(String, Node)>,
    /// Literals defined by bounds and ranges on one number are ordered.
    ordered: bool,
    /// Index of its attribute, once built.
    attribute: Option<usize>,
}

impl Concept {
    fn new(path: Vec<String>, kind: Kind) -> Self {
        Concept {
            path,
            kind,
            unit_type: String::new(),
            literals: vec![],
            definitions: vec![],
            ordered: false,
            attribute: None,
        }
    }

    fn name(&self) -> String {
        self.path.join(".")
    }
}

fn is_type_spec(value: &Node) -> bool {
    let Node::String(s) = value else { return false };
    match s.split_whitespace().next() {
        Some(head) => PRIMITIVES.contains(&head) || head == "boolean" || head == "shapefile",
        None => false,
    }
}

fn is_expression(value: &Node) -> bool {
    !value.is_mapping() && !is_type_spec(value)
}

fn count_keys(node: &Node, out: &mut HashMap<String, usize>) {
    if let Some(pairs) = node.as_mapping() {
        for (k, child) in pairs {
            *out.entry(k.py_str()).or_default() += 1;
            count_keys(child, out);
        }
    }
}

fn tail(key: &str) -> &str {
    key.rsplit('.').next().unwrap_or(key)
}

/// Whether `node` maps literals to expressions on concepts defined elsewhere.
///
/// A record whose fields are all categoricals looks the same, except that its
/// keys name its own fields: a literal's expressions name concepts outside it.
fn defines_literals(node: &Node, ids: &HashMap<String, usize>) -> bool {
    let Some(pairs) = node.as_mapping() else {
        return false;
    };
    let mut own = HashMap::new();
    count_keys(node, &mut own);
    pairs.len() >= 2
        && pairs.iter().all(|(_, v)| {
            let Some(inner) = v.as_mapping() else {
                return false;
            };
            !inner.is_empty()
                && inner
                    .iter()
                    .filter(|(k, _)| k.py_str() != "METADATA")
                    .all(|(k, _)| {
                        let t = k.py_str();
                        let t = tail(&t);
                        ids.get(t).copied().unwrap_or(0) > own.get(t).copied().unwrap_or(0)
                    })
                && inner.iter().all(|(_, x)| is_expression(x))
        })
}

fn flatten(
    node: &Node,
    prefix: &[String],
    ids: &HashMap<String, usize>,
    out: &mut IndexMap<String, Concept>,
    records: &BTreeSet<Vec<String>>,
) -> Result<()> {
    let path = prefix.join(".");
    match node {
        Node::Sequence(items) => {
            let mut c = Concept::new(prefix.to_vec(), Kind::Categorical);
            c.literals = items.iter().map(NodeExt::literal).collect();
            out.insert(path, c);
        }
        Node::Mapping(pairs) => {
            if !records.contains(prefix) && defines_literals(node, ids) {
                let mut c = Concept::new(prefix.to_vec(), Kind::Categorical);
                c.literals = pairs.iter().map(|(k, _)| k.literal()).collect();
                c.definitions = pairs
                    .iter()
                    .map(|(k, v)| (k.literal(), v.clone()))
                    .collect();
                out.insert(path, c);
                return Ok(());
            }
            for (key, child) in pairs {
                let mut p = prefix.to_vec();
                p.push(key.py_str());
                flatten(child, &p, ids, out, records)?;
            }
        }
        Node::String(s) => {
            let tokens: Vec<&str> = s.split_whitespace().collect();
            let head = tokens.first().copied().unwrap_or("");
            if PRIMITIVES.contains(&head) {
                let mut c = Concept::new(prefix.to_vec(), Kind::Number);
                c.unit_type = tokens[1..].join(" ");
                out.insert(path, c);
            } else if head == "boolean" && tokens.len() == 1 {
                out.insert(path, Concept::new(prefix.to_vec(), Kind::Boolean));
            } else if head == "shapefile" {
                // a shapefile is not read
            } else if tokens.len() == 1 && IDENTIFIER.is_match(head) {
                let mut c = Concept::new(prefix.to_vec(), Kind::Reference);
                c.unit_type = head.to_string();
                out.insert(path, c);
            } else {
                return Err(Error::open_odd(format!(
                    "TAXONOMY {path}: cannot read the type {} (OpenODD types: boolean, integer, long, \
                     float, double, a list, or a type's id)",
                    node.py_repr()
                )));
            }
        }
        other => {
            return Err(Error::open_odd(format!(
                "TAXONOMY {path}: cannot read {}",
                other.py_repr()
            )));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// The reader
// ---------------------------------------------------------------------------

struct Reader<'r> {
    resolver: &'r dyn Resolver,
    reader_keys: Vec<String>,
    name: String,
    text: String,
    units: Units,
    concepts: IndexMap<String, Concept>,
    module_names: BTreeSet<String>,
    labels: BTreeSet<String>,
    bindings: IndexMap<String, Node>,
    assumed_units: HashMap<String, String>,
    situations: Vec<(String, Node)>,
    attributes: Vec<Attribute>,
    modules_src: IndexMap<String, Node>,
    roots: Vec<String>,
}

fn conversions(block: &Mapping) -> Result<Vec<Conversion>> {
    let mut out = Vec::new();
    for (unit_type, sources) in block {
        let Some(sources) = sources.as_mapping() else {
            continue;
        };
        for (src, targets) in sources {
            let Some(targets) = targets.as_mapping() else {
                continue;
            };
            for (dst, spec) in targets {
                let get = |key: &str, default: f64| -> Result<f64> {
                    match spec.get(key) {
                        None => Ok(default),
                        Some(n) => n.to_f64().ok_or_else(|| {
                            Error::open_odd(format!(
                                "conversion {}: {key} must be a number",
                                unit_type.py_str()
                            ))
                        }),
                    }
                };
                out.push(Conversion {
                    unit_type: unit_type.py_str(),
                    from: src.py_str(),
                    to: dst.py_str(),
                    scale: get("scale", 1.0)?,
                    offset: get("offset", 0.0)?,
                });
            }
        }
    }
    Ok(out)
}

fn labels_of(mdef: &Node) -> Vec<String> {
    let mut labels = Vec::new();
    for key in ["LABEL", "LABELS"] {
        if let Some(n) = mdef.get(key) {
            labels.extend(n.as_list().iter().map(NodeExt::py_str));
        }
    }
    labels
}

impl<'r> Reader<'r> {
    fn new(docs: &Documents, options: &Options, resolver: &'r dyn Resolver) -> Result<Self> {
        let mut units = Units::new();
        units.add_conversions(&conversions(&docs.conversion)?);
        let taxonomy = Node::Mapping(docs.taxonomy.clone());
        let mut ids = HashMap::new();
        count_keys(&taxonomy, &mut ids);
        let mut reader = Reader {
            resolver,
            reader_keys: resolver.reader_keys(),
            name: options
                .name
                .clone()
                .or_else(|| docs.first_stem.clone())
                .unwrap_or_else(|| "openodd".into()),
            text: options.text.clone(),
            units,
            concepts: IndexMap::new(),
            module_names: docs.modules.keys().cloned().collect(),
            labels: BTreeSet::new(),
            bindings: IndexMap::new(),
            assumed_units: HashMap::new(),
            situations: Vec::new(),
            attributes: Vec::new(),
            modules_src: docs.modules.clone(),
            roots: docs.roots.clone(),
        };
        // A mapping of literals to expressions looks like a record of
        // categoricals: one whose "expressions" do not fit the concepts they
        // name is read again as a record.
        let mut records: BTreeSet<Vec<String>> = BTreeSet::new();
        loop {
            reader.concepts = IndexMap::new();
            flatten(&taxonomy, &[], &ids, &mut reader.concepts, &records)?;
            reader.resolve_references();
            let misread: Vec<Vec<String>> = reader
                .concepts
                .values()
                .filter(|c| !c.definitions.is_empty() && !reader.fits(c))
                .map(|c| c.path.clone())
                .collect();
            if misread.is_empty() {
                break;
            }
            records.extend(misread);
        }
        let keys: Vec<String> = reader.concepts.keys().cloned().collect();
        for key in keys {
            let ordered =
                reader.concepts[&key].ordered || reader.defined_by_ranges(&reader.concepts[&key]);
            reader.concepts[&key].ordered = ordered;
        }
        for mdef in docs.modules.values() {
            if mdef.is_mapping() {
                reader.labels.extend(labels_of(mdef));
            }
        }
        if let Some(bindings) = &options.bindings {
            if let Some(pairs) = bindings.as_mapping() {
                for (k, v) in pairs {
                    let name = reader.concept(&k.py_str(), "binding")?.name();
                    let v = if v.is_falsy() { empty_map() } else { v.clone() };
                    if !v.is_mapping() {
                        return Err(Error::open_odd(format!(
                            "probes.{name}: expected a mapping"
                        )));
                    }
                    reader.bindings.insert(name, v);
                }
            }
        }
        if let Some(s) = &options.situations {
            if let Some(pairs) = s.as_mapping() {
                reader.situations = pairs.iter().map(|(k, v)| (k.py_str(), v.clone())).collect();
            }
        }
        Ok(reader)
    }

    // -- names ----------------------------------------------------------------

    /// The concept `reference` names: its id, or a dotted tail of its path.
    fn concept(&self, reference: &str, where_: &str) -> Result<&Concept> {
        let suffix = format!(".{reference}");
        let found: Vec<&Concept> = self
            .concepts
            .values()
            .filter(|c| c.name() == reference || c.name().ends_with(&suffix))
            .collect();
        match found.len() {
            0 => Err(Error::open_odd(format!(
                "{where_}: {reference} is neither a concept nor a module or label"
            ))),
            1 => Ok(found[0]),
            _ => {
                let mut names: Vec<String> = found.iter().map(|c| c.name()).collect();
                names.sort();
                Err(Error::open_odd(format!(
                    "{where_}: {reference} is ambiguous ({}); give more of its path",
                    names.join(", ")
                )))
            }
        }
    }

    /// A concept typed by another concept's id takes its type.
    fn resolve_references(&mut self) {
        let keys: Vec<String> = self.concepts.keys().cloned().collect();
        for key in keys {
            let Some(concept) = self.concepts.get(&key) else {
                continue;
            };
            if concept.kind != Kind::Reference {
                continue;
            }
            let type_id = concept.unit_type.clone();
            let target = self
                .concepts
                .values()
                .find(|c| c.path.last() == Some(&type_id) && c.kind != Kind::Reference)
                .cloned();
            match target {
                None => {
                    self.concepts.shift_remove(&key);
                }
                Some(t) => {
                    let ordered = t.ordered || self.defined_by_ranges(&t);
                    let c = self.concepts.get_mut(&key).expect("still there");
                    c.kind = t.kind.clone();
                    c.literals = t.literals.clone();
                    c.unit_type = t.unit_type.clone();
                    c.ordered = ordered;
                }
            }
        }
    }

    /// The concept `reference` names outside `concept`; `None` if none or several.
    fn outside(&self, reference: &str, concept: &Concept) -> Option<&Concept> {
        let suffix = format!(".{reference}");
        let found: Vec<&Concept> = self
            .concepts
            .values()
            .filter(|c| {
                (c.name() == reference || c.name().ends_with(&suffix))
                    && c.path.get(..concept.path.len()) != Some(&concept.path[..])
            })
            .collect();
        if found.len() == 1 {
            Some(found[0])
        } else {
            None
        }
    }

    /// Whether each literal's expressions fit the concepts they name.
    fn fits(&self, concept: &Concept) -> bool {
        for (_, definition) in &concept.definitions {
            for (key, value) in definition.as_mapping().into_iter().flatten() {
                if key.py_str() == "METADATA" {
                    continue;
                }
                let Some(target) = self.outside(&key.py_str(), concept) else {
                    return false;
                };
                if target.path == concept.path {
                    return false;
                }
                match target.kind {
                    Kind::Number => match value {
                        Node::Number(_) => {}
                        Node::String(s) => {
                            let s = s.trim();
                            if !(RANGE_RE.is_match(s)
                                || BOUND_RE.is_match(s)
                                || QUANTITY_RE.is_match(s))
                            {
                                return false;
                            }
                        }
                        _ => return false,
                    },
                    Kind::Boolean => {
                        if !matches!(value, Node::Bool(_)) {
                            return false;
                        }
                    }
                    Kind::Categorical => {
                        let literals: Vec<String> = match value {
                            Node::Sequence(items) => items.iter().map(NodeExt::literal).collect(),
                            other => vec![other.literal()],
                        };
                        if target.definitions.is_empty()
                            && literals.iter().any(|x| !target.literals.contains(x))
                        {
                            return false;
                        }
                    }
                    Kind::Reference => {}
                }
            }
        }
        true
    }

    /// Whether every literal is a bound or range on one and the same number.
    fn defined_by_ranges(&self, concept: &Concept) -> bool {
        if concept.definitions.is_empty() {
            return false;
        }
        let mut targets = BTreeSet::new();
        for (_, definition) in &concept.definitions {
            let keys: Vec<(&Node, &Node)> = definition
                .as_mapping()
                .into_iter()
                .flatten()
                .filter(|(k, _)| k.py_str() != "METADATA")
                .collect();
            if keys.len() != 1 {
                return false;
            }
            let (key, value) = keys[0];
            let Some(target) = self.outside(&key.py_str(), concept) else {
                return false;
            };
            let Node::String(s) = value else { return false };
            let s = s.trim();
            if target.kind != Kind::Number || !(RANGE_RE.is_match(s) || BOUND_RE.is_match(s)) {
                return false;
            }
            targets.insert(target.name());
        }
        targets.len() == 1
    }

    // -- units -------------------------------------------------------------------

    fn binding(&self, name: &str) -> Node {
        self.bindings.get(name).cloned().unwrap_or(empty_map())
    }

    /// The backend reader a binding names (`measure:`), if it names one.
    fn reader_key(&self, concept: &str, binding: &Node) -> Result<Option<String>> {
        let named: Vec<String> = binding
            .as_mapping()
            .into_iter()
            .flatten()
            .map(|(k, _)| k.py_str())
            .filter(|k| self.reader_keys.contains(k))
            .collect();
        let probe = binding.has("probe");
        if named.len() + usize::from(probe) > 1 {
            let mut what = named.clone();
            if probe {
                what.push("probe".into());
            }
            what.sort();
            return Err(Error::open_odd(format!(
                "{concept}: give a probe or one reader, not {}",
                what.join(" and ")
            )));
        }
        Ok(named.into_iter().next())
    }

    fn unit_of(&self, concept: &Concept) -> Result<String> {
        let name = concept.name();
        let binding = self.binding(&name);
        if let Some(key) = self.reader_key(&name, &binding)? {
            let value = binding.get(&key).map(NodeExt::py_str).unwrap_or_default();
            let measured = normalize_unit(
                &self
                    .resolver
                    .reader_unit(&key, &value)
                    .map_err(|e| Error::open_odd(format!("probes.{name}: {e}")))?,
            );
            if let Some(given) = binding.get("unit") {
                let given = normalize_unit(&given.py_str());
                if !measured.is_empty() && given != measured {
                    return Err(Error::open_odd(format!(
                        "{name}: {key} {value} is in '{measured}', not '{given}'"
                    )));
                }
            }
            if !measured.is_empty() {
                return Ok(measured);
            }
        }
        if let Some(unit) = binding.get("unit") {
            return Ok(normalize_unit(&unit.py_str()));
        }
        if let Some(probe) = binding.get("probe") {
            let unit = self
                .resolver
                .probe_unit(&probe.py_str())
                .map_err(|e| Error::open_odd(format!("probes.{name}: probe: {e}")))?;
            return Ok(normalize_unit(&unit));
        }
        Ok(String::new())
    }

    fn number(
        &mut self,
        token: &str,
        unit: Option<&str>,
        concept_key: &str,
        where_: &str,
    ) -> Result<f64> {
        let concept = self.concepts[concept_key].clone();
        let name = concept.name();
        let value = crate::value::parse_float(token).ok_or_else(|| {
            Error::open_odd(format!(
                "{where_}: {name}: '{token}' is not a number (numeric terms and $parameters are not read)"
            ))
        })?;
        let mut to = self.unit_of(&concept)?;
        let unit = unit.filter(|u| !u.is_empty());
        if let Some(u) = unit {
            if self.units.unit_type(u).is_none() && u != to {
                return Err(Error::open_odd(format!(
                    "{where_}: {name}: unknown unit '{u}' (numeric terms and $parameters are not read)"
                )));
            }
            if to.is_empty() {
                // The probe's unit is unknown: numbers on this concept are read
                // in the first unit written, and another unit is converted into it.
                let measured = concept
                    .attribute
                    .map(|i| self.attributes[i].probe != Probe::Missing)
                    .unwrap_or(false);
                if measured {
                    return Err(Error::open_odd(format!(
                        "{where_}: {name}: '{u}' needs the probe's unit (binding 'unit:')"
                    )));
                }
                to = self
                    .assumed_units
                    .entry(name.clone())
                    .or_insert_with(|| normalize_unit(u))
                    .clone();
            }
            if !concept.unit_type.is_empty() {
                self.units
                    .check_type(u, &concept.unit_type)
                    .map_err(|e| Error::open_odd(format!("{where_}: {name}: {e}")))?;
            }
        }
        self.units
            .convert(value, unit.unwrap_or(""), &to)
            .map_err(|e| Error::open_odd(format!("{where_}: {name}: {e}")))
    }

    // -- attributes ---------------------------------------------------------------

    fn build_attributes(&mut self) -> Result<()> {
        // A derived categorical after every concept its expressions read.
        let mut pending: Vec<String> = self.concepts.keys().cloned().collect();
        while !pending.is_empty() {
            let ready: Vec<String> = pending
                .iter()
                .filter(|key| {
                    let c = &self.concepts[*key];
                    c.definitions.iter().all(|(_, d)| {
                        d.as_mapping()
                            .into_iter()
                            .flatten()
                            .filter(|(k, _)| k.py_str() != "METADATA")
                            .all(|(k, _)| {
                                self.outside(&k.py_str(), c)
                                    .is_some_and(|t| t.attribute.is_some())
                            })
                    })
                })
                .cloned()
                .collect();
            if ready.is_empty() {
                let names: Vec<String> = pending.iter().map(|k| self.concepts[k].name()).collect();
                return Err(Error::open_odd(format!(
                    "TAXONOMY: {} are defined by each other",
                    names.join(", ")
                )));
            }
            for key in ready {
                let attribute = self.attribute_of(&key)?;
                self.attributes.push(attribute);
                self.concepts[&key].attribute = Some(self.attributes.len() - 1);
                pending.retain(|k| *k != key);
            }
        }
        Ok(())
    }

    fn attribute_of(&mut self, key: &str) -> Result<Attribute> {
        let concept = self.concepts[key].clone();
        let name = concept.name();
        let binding = self.binding(&name);
        let probe = if let Some(reader) = self.reader_key(&name, &binding)? {
            let value = binding
                .get(&reader)
                .map(NodeExt::py_str)
                .unwrap_or_default();
            // validated through its unit
            self.resolver
                .reader_unit(&reader, &value)
                .map_err(|e| Error::open_odd(format!("probes.{name}: {e}")))?;
            Probe::External {
                kind: reader,
                spec: value,
            }
        } else if let Some(p) = binding.get("probe") {
            let spec = p.py_str();
            self.resolver
                .probe_unit(&spec)
                .map_err(|e| Error::open_odd(format!("probes.{name}: probe: {e}")))?;
            Probe::External {
                kind: "probe".into(),
                spec,
            }
        } else if !concept.definitions.is_empty() {
            self.derived_probe(key)?.unwrap_or(Probe::Missing)
        } else {
            Probe::Missing
        };
        let buckets = if let Some(values) = binding.get("values") {
            Some(Buckets::Values(
                values
                    .as_list()
                    .iter()
                    .map(|n| n.to_value().unwrap_or(Value::Str(n.py_str())))
                    .collect(),
            ))
        } else if let Some(edges) = binding.get("buckets") {
            let edges = edges
                .as_list()
                .iter()
                .map(|n| {
                    n.to_f64().ok_or_else(|| {
                        Error::open_odd(format!("binding {name}: buckets must be numbers"))
                    })
                })
                .collect::<Result<Vec<f64>>>()?;
            Some(Buckets::Edges(edges))
        } else if let Some(range) = binding.get("range") {
            let ends = range.as_list();
            let (Some(low), Some(high)) = (
                ends.first().and_then(NodeExt::to_f64),
                ends.get(1).and_then(NodeExt::to_f64),
            ) else {
                return Err(Error::open_odd(format!(
                    "binding {name}: range is [low, high]"
                )));
            };
            let every = binding
                .get("every")
                .and_then(NodeExt::to_f64)
                .unwrap_or(0.0);
            if every == 0.0 {
                return Err(Error::open_odd(format!(
                    "binding {name}: cover(odd.{name}): range needs every"
                )));
            }
            Some(Buckets::Range { low, high, every })
        } else if probe != Probe::Missing {
            match concept.kind {
                Kind::Categorical => Some(Buckets::Values(
                    concept
                        .literals
                        .iter()
                        .map(|l| Value::Str(l.clone()))
                        .collect(),
                )),
                Kind::Boolean => Some(Buckets::Values(vec![Value::Bool(false), Value::Bool(true)])),
                _ => None,
            }
        } else {
            None
        };
        let mut extra: Vec<String> = binding
            .as_mapping()
            .into_iter()
            .flatten()
            .map(|(k, _)| k.py_str())
            .filter(|k| !BINDING_KEYS.contains(&k.as_str()) && !self.reader_keys.contains(k))
            .collect();
        extra.sort();
        if !extra.is_empty() {
            return Err(Error::open_odd(format!(
                "probes.{name}: unknown keys {}",
                crate::model::py_list(&extra)
            )));
        }
        let criteria = criteria_of(
            &binding,
            &format!("probes.{name}"),
            "target and min_stay must be numbers",
        )?;
        let unit = self.unit_of(&concept)?;
        let text = binding.get("text").map(NodeExt::py_str).unwrap_or_default();
        let mut attribute = Attribute::new(name.clone(), probe, unit, text, criteria)
            .map_err(|e| Error::open_odd(format!("binding {name}: {e}")))?;
        if let Some(b) = buckets {
            attribute
                .set_buckets(b)
                .map_err(|e| Error::open_odd(format!("binding {name}: {e}")))?;
        }
        Ok(attribute)
    }

    /// A categorical defined by expressions; `None` when nothing it reads is measured.
    fn derived_probe(&mut self, key: &str) -> Result<Option<Probe>> {
        let concept = self.concepts[key].clone();
        let mut rules = Vec::new();
        for (literal, definition) in &concept.definitions {
            let where_ = format!("TAXONOMY {}.{literal}", concept.name());
            let conditions = self.conditions(definition, &where_, "", true, Some(&concept.path))?;
            rules.push((
                literal.clone(),
                Condition::all_of(conditions).map_err(|e| Error::open_odd(e.to_string()))?,
            ));
        }
        let mut sources = Vec::new();
        for (_, c) in &rules {
            c.attributes(&mut sources);
        }
        let all_missing = sources.iter().all(|s| {
            self.attributes
                .iter()
                .find(|a| a.name == *s)
                .is_none_or(|a| a.probe == Probe::Missing)
        });
        if all_missing {
            return Ok(None);
        }
        Ok(Some(Probe::Derived(rules)))
    }

    /// Give a measured number without buckets buckets at the thresholds tested,
    /// so each boundary the ODD draws is covered from both sides.
    fn threshold_buckets(&mut self, modules: &[Module]) -> Result<()> {
        let mut leaves: Vec<(String, Predicate)> = Vec::new();
        let mut collect = |c: &Condition| {
            let mut out = Vec::new();
            c.leaves(&mut out);
            leaves.extend(out.into_iter().map(|(a, p)| (a.to_string(), p.clone())));
        };
        for m in modules {
            m.conditions().for_each(&mut collect);
        }
        for a in &self.attributes {
            if let Probe::Derived(rules) = &a.probe {
                rules.iter().for_each(|(_, c)| collect(c));
            }
        }
        let mut edges: HashMap<String, BTreeSet<OrdF64>> = HashMap::new();
        for (attribute, predicate) in leaves {
            let found = edges.entry(attribute).or_default();
            match predicate {
                Predicate::Interval { low, high, .. } => {
                    for x in [low, high] {
                        if x.is_finite() {
                            found.insert(OrdF64(x));
                        }
                    }
                }
                Predicate::Equal(Value::Int(i)) => {
                    found.insert(OrdF64(i as f64));
                }
                Predicate::Equal(Value::Float(x)) => {
                    found.insert(OrdF64(x));
                }
                _ => {}
            }
        }
        let keys: Vec<String> = self.concepts.keys().cloned().collect();
        for key in keys {
            let c = &self.concepts[&key];
            let Some(i) = c.attribute else { continue };
            if c.kind != Kind::Number {
                continue;
            }
            let a = &self.attributes[i];
            if a.item.is_some() || a.probe == Probe::Missing {
                continue;
            }
            let Some(found) = edges.get(&a.name).filter(|e| !e.is_empty()) else {
                continue;
            };
            let mut e = vec![f64::NEG_INFINITY];
            e.extend(found.iter().map(|x| x.0));
            e.push(f64::INFINITY);
            let name = a.name.clone();
            self.attributes[i]
                .set_buckets(Buckets::Edges(e))
                .map_err(|err| Error::open_odd(format!("binding {name}: {err}")))?;
        }
        Ok(())
    }

    // -- conditions ----------------------------------------------------------------

    fn conditions(
        &mut self,
        section: &Node,
        where_: &str,
        op: &str,
        nested: bool,
        within: Option<&[String]>,
    ) -> Result<Vec<Condition>> {
        let pairs = match section.as_mapping() {
            Some(p) if p.iter().any(|(k, _)| k.py_str() != "METADATA") => p.clone(),
            _ => {
                return Err(Error::open_odd(format!(
                    "{where_}: a section maps concepts to expressions"
                )))
            }
        };
        let mut out = Vec::new();
        for (key, value) in pairs {
            let key = key.py_str();
            if key == "METADATA" {
                continue; // carries no semantics
            }
            if key == "AND" || key == "OR" {
                if nested {
                    return Err(Error::open_odd(format!(
                        "{where_}: sections nest one level only"
                    )));
                }
                if key == op {
                    let other = if op == "AND" { "OR" } else { "AND" };
                    return Err(Error::open_odd(format!(
                        "{where_}: an {op} section nests {other}, not {key}"
                    )));
                }
                let children = self.conditions(&value, where_, &key, true, within)?;
                out.push(if key == "AND" {
                    Condition::all_of(children)?
                } else {
                    Condition::any_of(children)?
                });
            } else if self.module_names.contains(&key) || self.labels.contains(&key) {
                let Node::Bool(b) = value else {
                    return Err(Error::open_odd(format!(
                        "{where_}: {key} is a module or label: true or false"
                    )));
                };
                out.push(Condition::module_holds(key, b)?);
            } else {
                out.push(self.leaf(&key, &value, where_, within)?);
            }
        }
        Ok(out)
    }

    fn concept_key(
        &self,
        reference: &str,
        where_: &str,
        _within: Option<&[String]>,
    ) -> Result<String> {
        Ok(self.concept(reference, where_)?.name())
    }

    fn leaf(
        &mut self,
        key: &str,
        value: &Node,
        where_: &str,
        within: Option<&[String]>,
    ) -> Result<Condition> {
        let ckey = self.concept_key(key, where_, within)?;
        let concept = self.concepts[&ckey].clone();
        let name = concept.name();
        let Some(index) = concept.attribute else {
            return Err(Error::open_odd(format!(
                "{where_}: {name} is defined by itself"
            )));
        };
        let attribute = self.attributes[index].name.clone();
        let unknown_word = |s: &str| {
            UNKNOWN.contains(&s.trim().to_lowercase().as_str())
                && !concept.literals.iter().any(|l| l == s.trim())
        };
        match value {
            Node::Null => return Ok(Condition::Missing { attribute }),
            Node::String(s) if unknown_word(s) => return Ok(Condition::Missing { attribute }),
            _ => {}
        }
        if concept.kind == Kind::Boolean {
            let Node::Bool(b) = value else {
                return Err(Error::open_odd(format!(
                    "{where_}: {name} is a boolean: true or false, not {}",
                    value.py_repr()
                )));
            };
            return Ok(Condition::Leaf {
                attribute,
                predicate: Predicate::Equal(Value::Bool(*b)),
            });
        }
        if let Node::Sequence(items) = value {
            if concept.kind == Kind::Number {
                return Err(Error::open_odd(format!(
                    "{where_}: {name} is a number: a list is for literals; a range is written \"[low .. high] unit\""
                )));
            }
            let literals: Vec<String> = items.iter().map(NodeExt::literal).collect();
            require_literals(&concept, &literals, where_)?;
            return is_in(attribute, &literals);
        }
        if concept.kind == Kind::Categorical && !matches!(value, Node::String(_)) {
            let literal = value.literal();
            require_literals(&concept, std::slice::from_ref(&literal), where_)?;
            return is_in(attribute, &[literal]);
        }
        if matches!(value, Node::Number(_) | Node::Bool(_)) {
            let x = self.number(&value.py_str(), None, &ckey, where_)?;
            return Ok(Condition::Leaf {
                attribute,
                predicate: Predicate::Equal(Value::Float(x)),
            });
        }
        let text = value.py_str().trim().to_string();
        if concept.kind == Kind::Number {
            return self.numeric(&ckey, attribute, &text, where_);
        }
        categorical(&concept, attribute, &text, where_)
    }

    fn numeric(
        &mut self,
        ckey: &str,
        attribute: String,
        text: &str,
        where_: &str,
    ) -> Result<Condition> {
        let name = self.concepts[ckey].name();
        if let Some(m) = RANGE_RE.captures(text) {
            let unit = m.name("unit").map(|u| u.as_str().to_string());
            let low = self.number(&m["low"], unit.as_deref(), ckey, where_)?;
            let high = self.number(&m["high"], unit.as_deref(), ckey, where_)?;
            if low > high {
                return Err(Error::open_odd(format!(
                    "{where_}: {name}: '{text}' ends before it starts"
                )));
            }
            return Ok(Condition::Leaf {
                attribute,
                predicate: Predicate::Interval {
                    low,
                    high,
                    include_low: true,
                    include_high: true,
                },
            });
        }
        if let Some(m) = BOUND_RE.captures(text) {
            let rest = m["rest"].trim().to_string();
            let Some(q) = QUANTITY_RE.captures(&rest) else {
                return Err(Error::open_odd(format!(
                    "{where_}: {name}: cannot read '{text}'"
                )));
            };
            let unit = q.name("unit").map(|u| u.as_str().to_string());
            let x = self.number(&q["value"], unit.as_deref(), ckey, where_)?;
            let (low, high, il, ih) = match &m["op"] {
                ">=" => (x, f64::INFINITY, true, true),
                ">" => (x, f64::INFINITY, false, true),
                "<=" => (f64::NEG_INFINITY, x, true, true),
                _ => (f64::NEG_INFINITY, x, true, false),
            };
            return Ok(Condition::Leaf {
                attribute,
                predicate: Predicate::Interval {
                    low,
                    high,
                    include_low: il,
                    include_high: ih,
                },
            });
        }
        if let Some(q) = QUANTITY_RE.captures(text) {
            let unit = q.name("unit").map(|u| u.as_str().to_string());
            let x = self.number(&q["value"], unit.as_deref(), ckey, where_)?;
            return Ok(Condition::Leaf {
                attribute,
                predicate: Predicate::Equal(Value::Float(x)),
            });
        }
        Err(Error::open_odd(format!(
            "{where_}: {name}: cannot read '{text}'"
        )))
    }

    // -- modules ---------------------------------------------------------------------

    fn build_modules(&mut self) -> Result<Vec<Module>> {
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
        let mut modules = Vec::new();
        for (name, mdef) in self.modules_src.clone() {
            let where_ = format!("module {name}");
            if !mdef.is_mapping() {
                return Err(Error::open_odd(format!("{where_}: expected a mapping")));
            }
            if name.to_lowercase().contains("unknown") {
                return Err(Error::open_odd(format!(
                    "{where_}: a module's id must not contain 'unknown'"
                )));
            }
            if self.concepts.values().any(|c| c.path.last() == Some(&name)) {
                return Err(Error::open_odd(format!(
                    "{where_}: a module's id must not be a concept's"
                )));
            }
            let mut extra: Vec<String> = mdef
                .as_mapping()
                .unwrap()
                .iter()
                .map(|(k, _)| k.py_str())
                .filter(|k| !MODULE_KEYS.contains(&k.as_str()))
                .collect();
            extra.sort();
            if !extra.is_empty() {
                return Err(Error::open_odd(format!(
                    "{where_}: unknown keys {}",
                    crate::model::py_list(&extra)
                )));
            }
            let includes: Vec<&str> = ["INCLUDE_AND", "INCLUDE_OR"]
                .into_iter()
                .filter(|k| mdef.has(k))
                .collect();
            let excludes: Vec<&str> = ["EXCLUDE_AND", "EXCLUDE_OR"]
                .into_iter()
                .filter(|k| mdef.has(k))
                .collect();
            if includes.len() > 1 || excludes.len() > 1 {
                return Err(Error::open_odd(format!(
                    "{where_}: at most one INCLUDE_* and one EXCLUDE_* section"
                )));
            }
            if includes.is_empty() && excludes.is_empty() {
                return Err(Error::open_odd(format!(
                    "{where_}: needs an INCLUDE_* or EXCLUDE_* section"
                )));
            }
            let mut section = |key: Option<&&str>| -> Result<Option<Section>> {
                let Some(key) = key else { return Ok(None) };
                let op = key.rsplit('_').next().unwrap_or("AND");
                let conditions = self.conditions(
                    mdef.get(key).unwrap(),
                    &format!("{where_} {key}"),
                    op,
                    false,
                    None,
                )?;
                Ok(Some(Section {
                    all: op == "AND",
                    conditions,
                }))
            };
            let include = section(includes.first())?;
            let exclude = section(excludes.first())?;
            let labels = labels_of(&mdef);
            if labels.iter().any(|l| l.to_lowercase().contains("unknown")) {
                return Err(Error::open_odd(format!(
                    "{where_}: a label must not contain 'unknown'"
                )));
            }
            let text = ["TITLE", "DESCRIPTION"]
                .iter()
                .filter_map(|k| mdef.get(k).filter(|v| !v.is_falsy()).map(NodeExt::py_str))
                .collect::<Vec<_>>()
                .join(" -- ");
            let mut module = Module::new(name.clone(), include, exclude)?;
            module.labels = labels;
            module.active = flag(
                mdef.get("ACTIVE").unwrap_or(&Node::Bool(true)),
                &format!("{where_} ACTIVE"),
            )?;
            module.text = text;
            if let Some(Node::Mapping(meta)) = mdef.get("METADATA") {
                module.metadata = meta
                    .iter()
                    .map(|(k, v)| (k.py_str(), v.to_json()))
                    .collect();
            }
            modules.push(module);
        }
        Ok(modules)
    }

    fn mark_situations(&self, modules: &mut [Module]) -> Result<()> {
        for (name, criteria) in &self.situations {
            let where_ = format!("situations.{name}");
            let Some(module) = modules.iter_mut().find(|m| m.name == *name) else {
                return Err(Error::open_odd(format!("{where_}: no module '{name}'")));
            };
            let criteria = if criteria.is_falsy() {
                empty_map()
            } else {
                criteria.clone()
            };
            if !criteria.is_mapping() {
                return Err(Error::open_odd(format!(
                    "{where_}: expected a mapping of criteria"
                )));
            }
            let mut extra: Vec<String> = criteria
                .as_mapping()
                .unwrap()
                .iter()
                .map(|(k, _)| k.py_str())
                .filter(|k| !matches!(k.as_str(), "target" | "cover_by" | "min_stay"))
                .collect();
            extra.sort();
            if !extra.is_empty() {
                return Err(Error::open_odd(format!(
                    "{where_}: unknown keys {}",
                    crate::model::py_list(&extra)
                )));
            }
            let c = criteria_of(&criteria, &where_, "target and min_stay must be numbers")?;
            module
                .as_situation(c)
                .map_err(|e| Error::open_odd(format!("{where_}: {e}")))?;
        }
        Ok(())
    }

    fn build(mut self) -> Result<Odd> {
        self.build_attributes()?;
        let mut modules = self.build_modules()?;
        self.mark_situations(&mut modules)?;
        self.threshold_buckets(&modules)?;
        let tails: BTreeSet<String> = self
            .concepts
            .values()
            .filter_map(|c| c.path.last().cloned())
            .collect();
        let clash: Vec<String> = self.labels.intersection(&tails).cloned().collect();
        if !clash.is_empty() {
            return Err(Error::open_odd(format!(
                "labels named like concepts: {}",
                crate::model::py_list(&clash)
            )));
        }
        // Modules under ODD are the root candidates.
        let roots = if self.roots.is_empty() {
            None
        } else {
            Some(self.roots.clone())
        };
        Odd::new(
            self.name.clone(),
            self.attributes,
            modules,
            roots,
            self.text.clone(),
        )
        .map_err(|e| Error::open_odd(e.to_string()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct OrdF64(f64);
impl Eq for OrdF64 {}
impl PartialOrd for OrdF64 {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for OrdF64 {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.total_cmp(&other.0)
    }
}

fn criteria_of(node: &Node, where_: &str, what: &str) -> Result<Criteria> {
    let mut c = Criteria::default();
    let bad = || Error::open_odd(format!("{where_}: {what}"));
    if let Some(t) = node.get("target") {
        c.target = t.to_f64().ok_or_else(bad)?;
    }
    if let Some(b) = node.get("cover_by") {
        c.cover_by = b.py_str();
    }
    if let Some(m) = node.get("min_stay").filter(|m| **m != Node::Null) {
        c.min_stay = Some(m.to_f64().ok_or_else(bad)?);
    }
    Ok(c)
}

fn is_in(attribute: String, literals: &[String]) -> Result<Condition> {
    if literals.is_empty() {
        return Err(Error::open_odd(format!("{attribute}.is_in(): no values")));
    }
    Ok(Condition::Leaf {
        attribute,
        predicate: Predicate::InSet {
            labels: literals.iter().cloned().collect(),
            numbers: vec![],
        },
    })
}

fn require_literals(concept: &Concept, literals: &[String], where_: &str) -> Result<()> {
    if concept.kind != Kind::Categorical {
        return Ok(());
    }
    let unknown: Vec<&String> = literals
        .iter()
        .filter(|x| !concept.literals.contains(x))
        .collect();
    if !unknown.is_empty() {
        return Err(Error::open_odd(format!(
            "{where_}: {} has no literal {} (literals: {})",
            concept.name(),
            unknown
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            concept.literals.join(", ")
        )));
    }
    Ok(())
}

fn categorical(
    concept: &Concept,
    attribute: String,
    text: &str,
    where_: &str,
) -> Result<Condition> {
    let name = concept.name();
    let range = RANGE_RE.captures(text);
    let bound = BOUND_RE.captures(text);
    if range.is_some() || bound.is_some() {
        if !concept.ordered {
            return Err(Error::open_odd(format!(
                "{where_}: {name}: '{text}' needs literals defined by ranges"
            )));
        }
        let order = &concept.literals;
        if let Some(m) = range {
            let (low, high) = (m["low"].trim().to_string(), m["high"].trim().to_string());
            require_literals(concept, &[low.clone(), high.clone()], where_)?;
            let i = order.iter().position(|x| *x == low).unwrap();
            let j = order.iter().position(|x| *x == high).unwrap();
            let (i, j) = (i.min(j), i.max(j));
            return is_in(attribute, &order[i..=j]);
        }
        let b = bound.unwrap();
        let reference = b["rest"].trim().to_string();
        require_literals(concept, std::slice::from_ref(&reference), where_)?;
        let i = order.iter().position(|x| *x == reference).unwrap();
        let chosen: &[String] = match &b["op"] {
            "<" => &order[..i],
            "<=" => &order[..=i],
            ">" => &order[i + 1..],
            _ => &order[i..],
        };
        if chosen.is_empty() {
            return Err(Error::open_odd(format!(
                "{where_}: {name} '{text}' holds for no literal"
            )));
        }
        return is_in(attribute, chosen);
    }
    if concept.kind == Kind::Categorical {
        require_literals(concept, &[text.to_string()], where_)?;
    }
    Ok(Condition::Leaf {
        attribute,
        predicate: Predicate::Equal(Value::Str(text.to_string())),
    })
}

fn flag(value: &Node, where_: &str) -> Result<bool> {
    if let Node::Bool(b) = value {
        return Ok(*b);
    }
    match value.py_str().trim().to_lowercase().as_str() {
        "true" | "yes" => Ok(true),
        "false" | "no" => Ok(false),
        _ => Err(Error::open_odd(format!(
            "{where_}: expected true or false, not {}",
            value.py_repr()
        ))),
    }
}

// ---------------------------------------------------------------------------
// Entry points
// ---------------------------------------------------------------------------

fn load_with(
    files: &Files,
    sources: &[Source],
    options: &Options,
    resolver: &dyn Resolver,
    entry: Option<String>,
) -> Result<Loaded> {
    if sources.is_empty() {
        return Err(Error::open_odd("load_openodd(): no sources"));
    }
    let mut docs = Documents::new(files);
    let mut reads: Vec<ReadSource> = Vec::new();
    for source in sources {
        match source {
            Source::Git(g) => {
                let path = match files {
                    Files::Disk => {
                        let (path, co) =
                            sources::checkout(g).map_err(|e| Error::open_odd(e.to_string()))?;
                        let root = co.root.canonicalize().unwrap_or(co.root.clone());
                        let origin = Origin {
                            git: g.url.clone(),
                            rev: g.rev.clone(),
                            commit: co.commit.clone(),
                            path: g.path.clone(),
                        };
                        docs.search
                            .push(path.parent().map(Path::to_path_buf).unwrap_or_default());
                        docs.search.push(root.clone());
                        docs.checkouts.push((root, origin.clone()));
                        docs.provenance.push(origin);
                        path
                    }
                    Files::Memory(t) => {
                        let name = Path::new(&g.path)
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        if let Some(o) = t.file(&name).and_then(|f| f.origin.clone()) {
                            docs.provenance.push(o);
                        }
                        PathBuf::from(MEMORY_ROOT).join(name)
                    }
                };
                reads.push(ReadSource::Path(path, None));
            }
            Source::Path(p) => {
                let p = files.resolve(p);
                docs.search
                    .push(p.parent().map(Path::to_path_buf).unwrap_or_default());
                reads.push(ReadSource::Path(p, None));
            }
            Source::Text(t) => {
                let looks_like_path = !t.contains('\n')
                    && (t.trim().ends_with(".yaml") || t.trim().ends_with(".yml"));
                if looks_like_path {
                    let p = files.resolve(Path::new(t.trim()));
                    reads.push(ReadSource::Path(p, None));
                } else {
                    reads.push(ReadSource::Text(t.clone()));
                }
            }
        }
    }
    for read in reads {
        docs.read(read, &mut Vec::new())?;
    }
    let reader = Reader::new(&docs, options, resolver)?;
    let root_names = docs.roots.clone();
    let odd = reader.build()?;
    let version = declared_version(&odd, &root_names)?;
    let mut transmission = docs.transmission;
    transmission.entry = entry.unwrap_or_else(|| {
        transmission
            .files
            .first()
            .map(|f| f.name.clone())
            .unwrap_or_default()
    });
    transmission.version = version.clone();
    Ok(Loaded {
        odd,
        transmission,
        sources: docs.provenance,
        version,
    })
}

/// The version the ODD's root modules declare in `METADATA` (`version: 1.2.0`).
fn declared_version(odd: &Odd, candidates: &[String]) -> Result<Option<String>> {
    let names: Vec<&String> = if candidates.is_empty() {
        odd.roots.iter().collect()
    } else {
        candidates.iter().collect()
    };
    let mut found: BTreeSet<String> = BTreeSet::new();
    for m in odd.modules.iter().filter(|m| names.contains(&&m.name)) {
        if let Some(v) = m.metadata.get("version") {
            found.insert(match v {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            });
        }
    }
    if found.len() > 1 {
        return Err(Error::open_odd(format!(
            "the root modules declare different versions: {}",
            found.into_iter().collect::<Vec<_>>().join(", ")
        )));
    }
    Ok(found.into_iter().next())
}

/// Read an ODD from OpenODD sources, with `options` for how it is measured.
pub fn load_openodd(
    sources: &[Source],
    options: &Options,
    resolver: &dyn Resolver,
) -> Result<Loaded> {
    load_with(&Files::Disk, sources, options, resolver, None)
}

fn binding_sources(path: &Path, doc: &Node) -> Result<Vec<Source>> {
    let mut out = Vec::new();
    for entry in doc.get("openodd").map(NodeExt::as_list).unwrap_or_default() {
        if let Some(pairs) = entry.as_mapping() {
            let where_ = path.display().to_string();
            let mut extra: Vec<String> = pairs
                .iter()
                .map(|(k, _)| k.py_str())
                .filter(|k| !matches!(k.as_str(), "git" | "rev" | "path"))
                .collect();
            extra.sort();
            if !extra.is_empty() {
                return Err(Error::open_odd(format!(
                    "{where_}: unknown keys {} in a git source",
                    crate::model::py_list(&extra)
                )));
            }
            let missing: Vec<String> = ["git", "rev", "path"]
                .iter()
                .filter(|k| entry.get(k).is_none_or(NodeExt::is_falsy))
                .map(|k| k.to_string())
                .collect();
            if !missing.is_empty() {
                return Err(Error::open_odd(format!(
                    "{where_}: a git source needs {}",
                    crate::model::py_list(&missing)
                )));
            }
            for p in entry.get("path").unwrap().as_list() {
                out.push(Source::Git(
                    GitSource::new(
                        entry.get("git").unwrap().py_str(),
                        entry.get("rev").unwrap().py_str(),
                        p.py_str(),
                    )
                    .map_err(|e| Error::open_odd(e.to_string()))?,
                ));
            }
        } else {
            out.push(Source::Path(
                path.parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_default()
                    .join(entry.py_str()),
            ));
        }
    }
    Ok(out)
}

fn load_binding_doc(
    files: &Files,
    path: &Path,
    data: &[u8],
    doc: &Node,
    resolver: &dyn Resolver,
) -> Result<Loaded> {
    let mut extra: Vec<String> = doc
        .as_mapping()
        .unwrap()
        .iter()
        .map(|(k, _)| k.py_str())
        .filter(|k| {
            !matches!(
                k.as_str(),
                "openodd" | "name" | "text" | "probes" | "situations"
            )
        })
        .collect();
    extra.sort();
    if !extra.is_empty() {
        return Err(Error::open_odd(format!(
            "{}: unknown keys {}",
            path.display(),
            crate::model::py_list(&extra)
        )));
    }
    let options = Options {
        bindings: doc.get("probes").cloned(),
        name: Some(
            doc.get("name")
                .filter(|n| !n.is_falsy())
                .map(NodeExt::py_str)
                .unwrap_or_else(|| stem(path)),
        ),
        text: doc.get("text").map(NodeExt::py_str).unwrap_or_default(),
        situations: doc.get("situations").cloned(),
    };
    let sources = binding_sources(path, doc)?;
    let mut loaded = load_with(files, &sources, &options, resolver, Some(file_name(path)))?;
    // The binding file is part of the definition, and the entry to it.
    let mut t = Transmission {
        entry: file_name(path),
        version: loaded.transmission.version.clone(),
        ..Default::default()
    };
    t.add(&file_name(path), data.to_vec(), None)?;
    for f in loaded.transmission.files.drain(..) {
        t.add(&f.name, f.data, f.origin)?;
    }
    loaded.transmission = t;
    Ok(loaded)
}

/// Read an ODD from a binding file: OpenODD files, and how to measure them.
///
/// ```yaml
/// openodd:                             # relative to this file,
///   - odd.yaml
///   - git: https://example.com/odd/taxonomy.git
///     rev: v1.2.0                      # or in git
///     path: taxonomy.yaml
/// name: urban                          # default: this file's stem
/// text: Urban roads, fair weather
/// probes:
///   speed_limit:                       # a concept, as conditions name it
///     probe: speed_limit_kph           # the backend's, or package.module:function
///     unit: km/h                       # what the probe returns
///     buckets: [0, 30, 60, 90]         # or values, or range + every
///     cover_by: meters                 # optional: hits, seconds, meters, entries
///     target: 200                      # optional
///     min_stay: 2                      # optional
/// situations:
///   close_cut_in: {target: 1}
/// ```
pub fn load_binding(path: &Path, resolver: &dyn Resolver) -> Result<Loaded> {
    load_file_with(&Files::Disk, path, resolver, true)
}

/// Read a `.yaml` ODD file: a binding file, or an OpenODD document on its own
/// (read with no probes, so nothing is measured).
pub fn load_file(path: &Path, resolver: &dyn Resolver) -> Result<Loaded> {
    load_file_with(&Files::Disk, path, resolver, false)
}

fn load_file_with(
    files: &Files,
    path: &Path,
    resolver: &dyn Resolver,
    binding_only: bool,
) -> Result<Loaded> {
    let path = files.resolve(path);
    let data = files.read(&path)?;
    let docs = parse_all(&String::from_utf8_lossy(&data))?;
    let is_binding = docs.len() == 1 && docs[0].is_mapping() && docs[0].has("openodd");
    if is_binding {
        return load_binding_doc(files, &path, &data, &docs[0], resolver);
    }
    if binding_only {
        return Err(Error::open_odd(format!(
            "{}: a binding file names its OpenODD files under 'openodd'",
            path.display()
        )));
    }
    let options = Options {
        name: Some(stem(&path)),
        ..Default::default()
    };
    let mut docs_reader = Documents::new(files);
    docs_reader
        .search
        .push(path.parent().map(Path::to_path_buf).unwrap_or_default());
    docs_reader.read(ReadSource::Path(path.clone(), Some(docs)), &mut Vec::new())?;
    let reader = Reader::new(&docs_reader, &options, resolver)?;
    let roots = docs_reader.roots.clone();
    let odd = reader.build()?;
    let version = declared_version(&odd, &roots)?;
    let mut transmission = docs_reader.transmission;
    transmission.entry = file_name(&path);
    transmission.version = version.clone();
    Ok(Loaded {
        odd,
        transmission,
        sources: docs_reader.provenance,
        version,
    })
}

/// Read an ODD back from its transmission (an archive's files), exactly as it
/// was read from the files it was made of.
pub fn load_transmission(transmission: &Transmission, resolver: &dyn Resolver) -> Result<Loaded> {
    let files = Files::Memory(transmission);
    let entry = PathBuf::from(MEMORY_ROOT).join(&transmission.entry);
    let mut loaded = load_file_with(&files, &entry, resolver, false)?;
    // Keep the origins the transmission recorded.
    for f in &mut loaded.transmission.files {
        if let Some(orig) = transmission.file(&f.name) {
            f.origin = orig.origin.clone();
        }
    }
    Ok(loaded)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Units1;
    impl Resolver for Units1 {
        fn reader_keys(&self) -> Vec<String> {
            vec!["measure".into()]
        }
        fn probe_unit(&self, spec: &str) -> std::result::Result<String, String> {
            match spec {
                "speed_kph" => Ok("km/h".into()),
                "rain" => Ok(String::new()),
                other => Err(format!("unknown probe '{other}'")),
            }
        }
        fn reader_unit(&self, _key: &str, value: &str) -> std::result::Result<String, String> {
            Ok(if value == "gap_m" {
                "m".into()
            } else {
                String::new()
            })
        }
    }

    const TAXONOMY: &str = r#"
TAXONOMY:
  speed: float velocity
  rain: [none, light, heavy]
  gap: float length
MODULES:
  slow:
    INCLUDE_AND:
      speed: "<= 60 km/h"
  dry:
    METADATA: {version: "1.2.0"}
    EXCLUDE_OR:
      rain: heavy
ODD:
  urban:
    METADATA: {version: "1.2.0"}
    INCLUDE_AND:
      slow: true
      dry: true
"#;

    fn write(dir: &Path, name: &str, text: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, text).unwrap();
        p
    }

    #[test]
    fn a_binding_file_reads_an_odd_and_its_transmission() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "odd.yaml", TAXONOMY);
        let binding = write(
            dir.path(),
            "urban.yaml",
            "openodd: [odd.yaml]\nprobes:\n  speed: {probe: speed_kph}\n  rain: {probe: rain}\n  gap: {measure: gap_m, buckets: [0, 10, 20]}\n",
        );
        let loaded = load_binding(&binding, &Units1).unwrap();
        let odd = &loaded.odd;
        assert_eq!(odd.name, "urban");
        assert_eq!(odd.roots, ["urban"]);
        assert_eq!(
            odd.attribute("speed")
                .unwrap()
                .item
                .as_ref()
                .unwrap()
                .labels,
            ["[-inf, 60)", "[60, inf]"]
        );
        assert_eq!(odd.outside_buckets("rain"), ["heavy"]);
        assert_eq!(loaded.version.as_deref(), Some("1.2.0"));
        let names: Vec<&str> = loaded
            .transmission
            .files
            .iter()
            .map(|f| f.name.as_str())
            .collect();
        assert_eq!(names, ["urban.yaml", "odd.yaml"]);
        assert_eq!(loaded.transmission.entry, "urban.yaml");

        // The archive reads back to the same ODD.
        let archive = loaded.transmission.to_archive().unwrap();
        let back =
            load_transmission(&Transmission::from_archive(&archive).unwrap(), &Units1).unwrap();
        assert_eq!(back.revision(), loaded.revision());
        assert_eq!(back.odd.structure_digest(), loaded.odd.structure_digest());
    }

    #[test]
    fn units_convert_into_the_probes() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "odd.yaml", TAXONOMY);
        let binding = write(
            dir.path(),
            "b.yaml",
            "openodd: [odd.yaml]\nprobes:\n  speed: {probe: speed_kph}\n",
        );
        let odd = load_binding(&binding, &Units1).unwrap().odd;
        let mut v = crate::model::Values::new();
        v.insert("speed".into(), Value::Float(59.0));
        assert!(odd.evaluate(&v).inside);
        v.insert("speed".into(), Value::Float(61.0));
        assert!(!odd.evaluate(&v).inside);
    }

    #[test]
    fn an_unknown_probe_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "odd.yaml", TAXONOMY);
        let binding = write(
            dir.path(),
            "b.yaml",
            "openodd: [odd.yaml]\nprobes:\n  speed: {probe: nope}\n",
        );
        let err = load_binding(&binding, &Units1).unwrap_err();
        assert!(err.to_string().contains("unknown probe 'nope'"), "{err}");
    }

    #[test]
    fn a_categorical_defined_by_ranges_is_derived_and_ordered() {
        let text = r#"
TAXONOMY:
  rate: float precipitation_rate
  level:
    dry: {rate: "< 0.1 mm/h"}
    light: {rate: "[0.1 .. 2.5] mm/h"}
    heavy: {rate: "> 2.5 mm/h"}
MODULES:
  ok:
    INCLUDE_AND:
      level: "<= light"
"#;
        let loaded = load_openodd(
            &[Source::Text(text.into())],
            &Options {
                bindings: Some(
                    parse_all("rate: {probe: rain, unit: mm/h}")
                        .unwrap()
                        .remove(0),
                ),
                ..Default::default()
            },
            &Units1,
        )
        .unwrap();
        let odd = loaded.odd;
        let mut v = crate::model::Values::new();
        v.insert("rate".into(), Value::Float(5.0));
        odd.complete(&mut v);
        assert_eq!(v["level"], Value::Str("heavy".into()));
        assert!(!odd.evaluate(&v).inside);
        assert_eq!(odd.outside_buckets("level"), ["heavy"]);
    }
}
