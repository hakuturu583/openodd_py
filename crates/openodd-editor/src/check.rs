//! What openodd's reader makes of the document: the same reader the Python
//! package uses, run in the browser on the edited text and the files it
//! imports.

use openodd::loader::AnyProbe;

use crate::doc::{Cond, CondValue, Document, Kind};
use openodd::model::{Odd, Truth};
use openodd::{load_transmission, Loaded, Transmission, Value, Values};

/// The editor's file and the files it imports, by name.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Files {
    pub entry: String,
    pub text: String,
    pub library: Vec<(String, String)>,
}

/// Where a bucket (or a cell of two) lies, the other attributes aside.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum State {
    /// Some values of the others put it inside the ODD.
    Inside,
    /// Outside the ODD whatever the others are.
    Outside,
}

impl State {
    pub fn class(self) -> &'static str {
        match self {
            State::Inside => "inside",
            State::Outside => "outside",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Attribute {
    pub name: String,
    pub kind: String,
    pub unit: String,
    pub buckets: Vec<String>,
    /// Each bucket's state, in order.
    pub states: Vec<State>,
    pub outside: Vec<String>,
    /// Bucket edges of a number, ascending (empty for a categorical).
    pub edges: Vec<f64>,
    /// The values a categorical takes (empty for a number).
    pub values: Vec<String>,
    pub derived: bool,
    /// A number of an integer primitive (`integer`, `long`).
    pub integer: bool,
}

impl Attribute {
    pub fn is_number(&self) -> bool {
        self.kind == "number"
    }
}

/// A module's arrow to what it tests: another module or label, or an attribute.
#[derive(Clone, Debug, PartialEq)]
pub struct Link {
    pub target: String,
    pub include: bool,
    pub module: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Module {
    pub name: String,
    pub root: bool,
    pub active: bool,
    pub labels: Vec<String>,
    pub include: Option<(bool, Vec<String>)>,
    pub exclude: Option<(bool, Vec<String>)>,
    pub links: Vec<Link>,
}

/// The ODD read, shared: equal only to itself, so a new read is a change.
#[derive(Clone, Debug)]
pub struct Shared(pub std::sync::Arc<Odd>);

impl PartialEq for Shared {
    fn eq(&self, other: &Self) -> bool {
        std::sync::Arc::ptr_eq(&self.0, &other.0)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    pub name: String,
    pub revision: String,
    pub version: Option<String>,
    pub roots: Vec<String>,
    pub attributes: Vec<Attribute>,
    pub modules: Vec<Module>,
    pub odd: Shared,
}

impl Report {
    pub fn attribute(&self, name: &str) -> Option<&Attribute> {
        self.attributes.iter().find(|a| a.name == name)
    }

    /// The modules carrying `label`.
    pub fn labelled(&self, label: &str) -> Vec<&str> {
        self.modules
            .iter()
            .filter(|m| m.labels.iter().any(|l| l == label))
            .map(|m| m.name.as_str())
            .collect()
    }
}

/// A backend that measures everything, a probe's unit being its name: the
/// checker measures every concept, in the unit the document writes it in.
struct Measured;

impl openodd::Resolver for Measured {
    fn probe_unit(&self, spec: &str) -> std::result::Result<String, String> {
        Ok(spec.to_string())
    }
}

const BINDING: &str = "openodd-editor.binding.yaml";

fn transmission(files: &Files, binding: Option<String>) -> Result<Transmission, String> {
    let mut t = Transmission::default();
    let entry = if files.entry.is_empty() {
        "odd.yaml"
    } else {
        &files.entry
    };
    t.add(entry, files.text.as_bytes().to_vec(), None)
        .map_err(|e| e.to_string())?;
    for (name, text) in &files.library {
        if name != entry {
            t.add(name, text.as_bytes().to_vec(), None)
                .map_err(|e| e.to_string())?;
        }
    }
    t.entry = entry.to_string();
    if let Some(binding) = binding {
        t.add(BINDING, binding.into_bytes(), None)
            .map_err(|e| e.to_string())?;
        t.entry = BINDING.into();
    }
    Ok(t)
}

/// What the reader makes of the files, every concept measured.
///
/// Read once as it is (nothing measured: the units numbers are written in),
/// then with a binding that measures each concept in its unit, so that the
/// reader buckets every attribute and works out which buckets are outside.
pub fn load(files: &Files) -> Result<Loaded, String> {
    let plain =
        load_transmission(&transmission(files, None)?, &AnyProbe).map_err(|e| e.to_string())?;
    let entry = if files.entry.is_empty() {
        "odd.yaml"
    } else {
        &files.entry
    };
    let (units, derived, _) = hints(files);
    let mut probes = serde_norway::Mapping::new();
    for a in plain
        .odd
        .attributes
        .iter()
        .filter(|a| !a.derived() && !derived.contains(&a.name))
    {
        let tail = a.name.rsplit('.').next().unwrap_or(&a.name);
        let unit = units
            .iter()
            .find(|(k, _)| *k == a.name)
            .or_else(|| units.iter().find(|(k, _)| k == tail))
            .map(|(_, u)| u.clone())
            .unwrap_or_default();
        let mut b = serde_norway::Mapping::new();
        b.insert("probe".into(), unit.into());
        probes.insert(a.name.clone().into(), b.into());
    }
    let mut binding = serde_norway::Mapping::new();
    binding.insert("openodd".into(), vec![entry.to_string()].into());
    binding.insert("name".into(), plain.odd.name.clone().into());
    binding.insert("probes".into(), probes.into());
    let binding = serde_norway::to_string(&binding).map_err(|e| e.to_string())?;
    let mut loaded = load_transmission(&transmission(files, Some(binding))?, &Measured)
        .map_err(|e| e.to_string())?;
    // The revision is the document's, not the checker's binding's.
    loaded.transmission = plain.transmission;
    Ok(loaded)
}

/// What the files say of their concepts: the unit each one's numbers are
/// first written in (key as written -> unit), and the derived categoricals,
/// which are not measured but worked out.
fn hints(files: &Files) -> (Vec<(String, String)>, Vec<String>, Vec<String>) {
    let mut out: Vec<(String, String)> = vec![];
    let mut derived = vec![];
    let mut integers = vec![];
    let texts = std::iter::once(&files.text).chain(files.library.iter().map(|(_, t)| t));
    for text in texts {
        let Ok(doc) = Document::from_yaml(text) else {
            continue;
        };
        let mut lists: Vec<&Vec<Cond>> = vec![];
        for m in &doc.modules {
            for s in [&m.include, &m.exclude].into_iter().flatten() {
                lists.push(&s.conditions);
                lists.push(&s.nested);
            }
        }
        fn defs<'a>(cs: &'a [crate::doc::Concept], out: &mut Vec<&'a Vec<Cond>>) {
            for c in cs {
                match &c.kind {
                    Kind::Group(children) => defs(children, out),
                    Kind::Derived(d) => out.extend(d.iter().map(|d| &d.conditions)),
                    _ => {}
                }
            }
        }
        defs(&doc.taxonomy, &mut lists);
        derived.extend(
            doc.leaves()
                .into_iter()
                .filter(|(_, k)| matches!(k, Kind::Derived(_)))
                .map(|(n, _)| n),
        );
        integers.extend(
            doc.leaves()
                .into_iter()
                .filter(|(_, k)| matches!(k, Kind::Number { primitive, .. } if primitive == "integer" || primitive == "long"))
                .map(|(n, _)| n),
        );
        for c in lists.into_iter().flatten() {
            let unit = match &c.value {
                CondValue::Range { unit, .. } => unit.clone(),
                CondValue::Compare { operand, .. } => {
                    match operand.split_whitespace().collect::<Vec<_>>()[..] {
                        [n, u] if n.parse::<f64>().is_ok() => u.to_string(),
                        _ => String::new(),
                    }
                }
                _ => String::new(),
            };
            if !unit.is_empty() && !out.iter().any(|(k, _)| *k == c.key) {
                out.push((c.key.clone(), unit));
            }
        }
    }
    (out, derived, integers)
}

pub fn check(files: &Files) -> Result<Report, String> {
    let loaded = load(files)?;
    let odd = &loaded.odd;
    let (_, _, integers) = hints(files);
    let attributes = odd
        .attributes
        .iter()
        .map(|a| {
            let item = a.item.as_ref();
            let values: Vec<String> = item
                .map(|i| i.values.iter().map(value_text).collect())
                .unwrap_or_default();
            let kind = if a.derived() {
                "derived"
            } else if item.is_some_and(|i| !i.edges.is_empty()) {
                "number"
            } else if values.iter().all(|v| v == "True" || v == "False") && !values.is_empty() {
                "boolean"
            } else if values.is_empty() {
                "number"
            } else {
                "categorical"
            };
            let buckets = item.map(|i| i.labels.clone()).unwrap_or_default();
            Attribute {
                name: a.name.clone(),
                kind: kind.into(),
                unit: a.unit.clone(),
                states: (0..buckets.len())
                    .map(|i| state(odd, &[(&a.name, i)]))
                    .collect(),
                buckets,
                outside: odd.outside_buckets(&a.name),
                edges: item.map(|i| i.edges.clone()).unwrap_or_default(),
                values,
                derived: a.derived(),
                integer: integers.contains(&a.name),
            }
        })
        .collect();
    let modules = odd
        .modules
        .iter()
        .map(|m| {
            let section = |s: &Option<openodd::Section>| {
                s.as_ref().map(|s| {
                    (
                        s.all,
                        s.conditions
                            .iter()
                            .map(|c| c.describe())
                            .collect::<Vec<_>>(),
                    )
                })
            };
            let mut links: Vec<Link> = vec![];
            for (include, s) in [(true, &m.include), (false, &m.exclude)] {
                let Some(s) = s else { continue };
                let mut refs = std::collections::BTreeSet::new();
                let mut attrs = vec![];
                for c in &s.conditions {
                    c.references(&mut refs);
                    c.attributes(&mut attrs);
                }
                let mut push = |target: String, module: bool| {
                    if !links
                        .iter()
                        .any(|l| l.target == target && l.include == include)
                    {
                        links.push(Link {
                            target,
                            include,
                            module,
                        });
                    }
                };
                for r in refs {
                    push(r, true);
                }
                for a in attrs {
                    push(a, false);
                }
            }
            Module {
                name: m.name.clone(),
                root: odd.roots.contains(&m.name),
                active: m.active,
                labels: m.labels.clone(),
                include: section(&m.include),
                exclude: section(&m.exclude),
                links,
            }
        })
        .collect();
    Ok(Report {
        name: odd.name.clone(),
        revision: loaded.revision(),
        version: loaded.version.clone(),
        roots: odd.roots.clone(),
        attributes,
        modules,
        odd: Shared(std::sync::Arc::new(loaded.odd)),
    })
}

/// Where the ODD stands with these attributes in these buckets: outside when
/// it is outside whatever the others are.
fn state(odd: &Odd, at: &[(&str, usize)]) -> State {
    let mut open: Values = odd
        .attributes
        .iter()
        .map(|b| (b.name.clone(), Value::Open))
        .collect();
    for (name, i) in at {
        open.insert(name.to_string(), Value::Bucket(*i));
    }
    odd.complete_open(&mut open);
    if odd.evaluate(&open).inside {
        State::Inside
    } else {
        State::Outside
    }
}

/// What one module makes of each bucket of an attribute, the others aside:
/// outside when the module cannot hold there.
pub fn module_states(report: &Report, module: &str, attribute: &str) -> Vec<State> {
    let odd = &report.odd.0;
    let Some(a) = report.attribute(attribute) else {
        return vec![];
    };
    (0..a.buckets.len())
        .map(|i| {
            let mut open: Values = odd
                .attributes
                .iter()
                .map(|b| (b.name.clone(), Value::Open))
                .collect();
            open.insert(attribute.to_string(), Value::Bucket(i));
            odd.complete_open(&mut open);
            match odd.evaluate(&open).modules.get(module) {
                Some(Truth::False) => State::Outside,
                _ => State::Inside,
            }
        })
        .collect()
}

/// Where the ODD (or one module) stands at each of these values of a numeric
/// attribute, the others aside: what a number line draws.
pub fn profile(report: &Report, attribute: &str, module: Option<&str>, xs: &[f64]) -> Vec<State> {
    let odd = &report.odd.0;
    let open: Values = odd
        .attributes
        .iter()
        .map(|b| (b.name.clone(), Value::Open))
        .collect();
    xs.iter()
        .map(|x| {
            let mut values = open.clone();
            values.insert(attribute.to_string(), Value::Float(*x));
            odd.complete_open(&mut values);
            let v = odd.evaluate(&values);
            let inside = match module {
                None => v.inside,
                Some(m) => v.modules.get(m) != Some(&Truth::False),
            };
            if inside {
                State::Inside
            } else {
                State::Outside
            }
        })
        .collect()
}

/// The states of every pair of buckets of two attributes: `rows[i][j]` for
/// `y`'s bucket `i` and `x`'s bucket `j`.
pub fn matrix(report: &Report, x: &str, y: &str) -> Vec<Vec<State>> {
    let (Some(ax), Some(ay)) = (report.attribute(x), report.attribute(y)) else {
        return vec![];
    };
    (0..ay.buckets.len())
        .map(|i| {
            (0..ax.buckets.len())
                .map(|j| state(&report.odd.0, &[(x, j), (y, i)]))
                .collect()
        })
        .collect()
}

fn value_text(v: &Value) -> String {
    match v {
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::Int(i) => i.to_string(),
        Value::Float(f) => f.to_string(),
        Value::Str(s) => s.clone(),
        Value::Missing => "unknown".into(),
        Value::Open => "*".into(),
        Value::Bucket(i) => format!("#{i}"),
    }
}

fn truth_text(t: &Truth) -> &'static str {
    match t {
        Truth::True => "true",
        Truth::False => "false",
        Truth::Unknown => "unknown",
        Truth::Inactive => "inactive",
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Evaluation {
    pub inside: bool,
    pub assumed: bool,
    /// Every module's and label's verdict, and the attributes derived from the values.
    pub modules: Vec<(String, String)>,
    pub derived: Vec<(String, String)>,
}

impl Evaluation {
    pub fn truth(&self, name: &str) -> Option<&str> {
        self.modules
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, t)| t.as_str())
    }
}

/// Judge values (attribute -> text; empty: unknown) against the ODD.
pub fn evaluate(report: &Report, inputs: &[(String, String)]) -> Evaluation {
    let odd = &report.odd.0;
    let mut values = Values::new();
    for (name, text) in inputs {
        let text = text.trim();
        let Some(attr) = odd.attribute(name) else {
            continue;
        };
        if text.is_empty() || attr.derived() {
            continue;
        }
        let numeric = attr.item.as_ref().is_some_and(|i| !i.edges.is_empty())
            || attr.item.as_ref().is_none_or(|i| i.values.is_empty());
        let value = if text.eq_ignore_ascii_case("true") {
            Value::Bool(true)
        } else if text.eq_ignore_ascii_case("false") {
            Value::Bool(false)
        } else if let (true, Ok(f)) = (numeric, text.parse::<f64>()) {
            Value::Float(f)
        } else if let Ok(i) = text.parse::<i64>() {
            Value::Int(i)
        } else {
            Value::Str(text.to_string())
        };
        values.insert(name.clone(), value);
    }
    odd.complete(&mut values);
    let verdict = odd.evaluate(&values);
    let derived = odd
        .attributes
        .iter()
        .filter(|a| a.derived())
        .map(|a| {
            (
                a.name.clone(),
                values
                    .get(&a.name)
                    .map(value_text)
                    .unwrap_or("unknown".into()),
            )
        })
        .collect();
    Evaluation {
        inside: verdict.inside,
        assumed: verdict.assumed,
        modules: verdict
            .modules
            .iter()
            .map(|(name, t)| (name.clone(), truth_text(t).to_string()))
            .collect(),
        derived,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const URBAN: &str = include_str!("../examples/urban.yaml");

    fn files(text: &str) -> Files {
        Files {
            entry: "urban.yaml".into(),
            text: text.into(),
            library: vec![],
        }
    }

    #[test]
    fn the_example_checks() {
        let r = check(&files(URBAN)).unwrap();
        assert_eq!(r.roots, ["urban_shuttle"]);
        assert_eq!(r.version.as_deref(), Some("1.0.0"));
        let fog = r
            .attributes
            .iter()
            .find(|a| a.name == "environment.fog")
            .unwrap();
        assert_eq!(fog.outside, ["dense"]);
        let rain = r
            .attributes
            .iter()
            .find(|a| a.name == "environment.rainfall")
            .unwrap();
        assert!(rain.derived);
        assert_eq!(fog.states, [State::Inside, State::Inside, State::Outside]);
        let speed = r.attribute("dynamic.ego_speed").unwrap();
        assert_eq!(speed.states[0], State::Outside);
        let root = r
            .modules
            .iter()
            .find(|m| m.name == "urban_shuttle")
            .unwrap();
        assert!(root
            .links
            .iter()
            .any(|l| l.target == "weather" && !l.include && l.module));
        assert!(root
            .links
            .iter()
            .any(|l| l.target == "dynamic.ego_speed" && l.include && !l.module));
        let m = matrix(&r, "environment.fog", "dynamic.ego_speed");
        assert_eq!(m.len(), speed.buckets.len());
        assert_eq!(m[1][2], State::Outside);
        assert_eq!(m[1][0], State::Inside);
        assert!(r.attribute("scenery.lane_count").unwrap().integer);
        let lanes = profile(
            &r,
            "scenery.lane_count",
            Some("town_roads"),
            &[1.0, 2.0, 3.0],
        );
        assert_eq!(lanes, [State::Inside, State::Inside, State::Outside]);
        let speed = profile(&r, "dynamic.ego_speed", None, &[-1.0, 10.0, 50.0]);
        assert_eq!(speed, [State::Outside, State::Inside, State::Outside]);
        let dark = module_states(&r, "dark", "environment.illumination");
        assert_eq!(dark, [State::Outside, State::Outside, State::Inside]);
    }

    #[test]
    fn a_wrong_document_says_why() {
        let err = check(&files(
            "TAXONOMY:\n  a: [x]\nODD:\n  m:\n    INCLUDE_AND: {b: x}\n",
        ))
        .unwrap_err();
        assert!(err.contains('b'), "{err}");
    }

    #[test]
    fn imports_are_read_from_the_library() {
        let mut f = files("IMPORT: [tax.yaml]\nODD:\n  m:\n    INCLUDE_AND: {a: x}\n");
        f.library
            .push(("tax.yaml".into(), "TAXONOMY:\n  a: [x, y]\n".into()));
        let r = check(&f).unwrap();
        assert_eq!(r.attributes[0].outside, ["y"]);
        f.library.clear();
        assert!(check(&f).is_err());
    }

    #[test]
    fn values_are_judged() {
        let f = files(URBAN);
        let mut inputs = vec![
            ("scenery.road_type".to_string(), "local".to_string()),
            ("scenery.lane_count".into(), "2".into()),
            ("scenery.junction".into(), "false".into()),
            ("scenery.speed_limit".into(), "40".into()),
            ("environment.illumination".into(), "day".into()),
            ("environment.rainfall_rate".into(), "0".into()),
            ("environment.fog".into(), "none".into()),
            ("dynamic.ego_speed".into(), "30".into()),
        ];
        let r = check(&f).unwrap();
        let e = evaluate(&r, &inputs);
        assert!(e.inside, "{e:?}");
        assert!(e
            .derived
            .contains(&("environment.rainfall".into(), "dry".into())));
        inputs[6].1 = "dense".into();
        assert!(!evaluate(&r, &inputs).inside);
    }
}
