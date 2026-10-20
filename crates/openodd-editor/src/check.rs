//! What openodd's reader makes of the document: the same reader the Python
//! package uses, run in the browser on the edited text and the files it
//! imports.

use openodd::loader::AnyProbe;

use crate::doc::{Cond, CondValue, Document, Kind};
use openodd::model::Truth;
use openodd::{load_transmission, Loaded, Transmission, Value, Values};

/// The editor's file and the files it imports, by name.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Files {
    pub entry: String,
    pub text: String,
    pub library: Vec<(String, String)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Attribute {
    pub name: String,
    pub kind: String,
    pub unit: String,
    pub buckets: Vec<String>,
    pub outside: Vec<String>,
    /// The values a categorical takes (empty for a number).
    pub values: Vec<String>,
    pub derived: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Module {
    pub name: String,
    pub root: bool,
    pub active: bool,
    pub labels: Vec<String>,
    pub include: Option<(bool, Vec<String>)>,
    pub exclude: Option<(bool, Vec<String>)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    pub name: String,
    pub revision: String,
    pub version: Option<String>,
    pub roots: Vec<String>,
    pub attributes: Vec<Attribute>,
    pub modules: Vec<Module>,
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
    let (units, derived) = hints(files);
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
fn hints(files: &Files) -> (Vec<(String, String)>, Vec<String>) {
    let mut out: Vec<(String, String)> = vec![];
    let mut derived = vec![];
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
    (out, derived)
}

pub fn check(files: &Files) -> Result<Report, String> {
    let loaded = load(files)?;
    let odd = &loaded.odd;
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
            Attribute {
                name: a.name.clone(),
                kind: kind.into(),
                unit: a.unit.clone(),
                buckets: item.map(|i| i.labels.clone()).unwrap_or_default(),
                outside: odd.outside_buckets(&a.name),
                values,
                derived: a.derived(),
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
            Module {
                name: m.name.clone(),
                root: odd.roots.contains(&m.name),
                active: m.active,
                labels: m.labels.clone(),
                include: section(&m.include),
                exclude: section(&m.exclude),
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
    })
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

#[derive(Clone, Debug, PartialEq)]
pub struct Evaluation {
    pub inside: bool,
    pub assumed: bool,
    /// Every module's verdict, and the attributes derived from the values.
    pub modules: Vec<(String, String)>,
    pub derived: Vec<(String, String)>,
}

/// Judge values (attribute -> text; empty: unknown) against the ODD.
pub fn evaluate(files: &Files, inputs: &[(String, String)]) -> Result<Evaluation, String> {
    let odd = load(files)?.odd;
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
    Ok(Evaluation {
        inside: verdict.inside,
        assumed: verdict.assumed,
        modules: verdict
            .modules
            .iter()
            .map(|(name, t)| {
                let t = match t {
                    Truth::True => "true",
                    Truth::False => "false",
                    Truth::Unknown => "unknown",
                    Truth::Inactive => "inactive",
                };
                (name.clone(), t.to_string())
            })
            .collect(),
        derived,
    })
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
        let e = evaluate(&f, &inputs).unwrap();
        assert!(e.inside, "{e:?}");
        assert!(e
            .derived
            .contains(&("environment.rainfall".into(), "dry".into())));
        inputs[6].1 = "dense".into();
        assert!(!evaluate(&f, &inputs).unwrap().inside);
    }
}
