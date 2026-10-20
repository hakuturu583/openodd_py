//! What openodd's reader makes of the document: the same reader the Python
//! package uses, run in the browser on the edited text and the files it
//! imports.

use openodd::loader::AnyProbe;
use openodd::model::{Odd, Truth, Verdict};
use openodd::value::parse_float;
use openodd::{load_transmission, ConceptKind, Loaded, Transmission, Value, Values};

use crate::doc::{tail, KindTag};

/// The editor's file and the files it imports, by name.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Files {
    pub entry: String,
    pub text: String,
    pub library: Vec<(String, String)>,
}

impl Files {
    fn entry(&self) -> &str {
        if self.entry.is_empty() {
            "odd.yaml"
        } else {
            &self.entry
        }
    }
}

/// Where a value (a bucket, a cell of two) lies, the other attributes aside.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum State {
    /// Some values of the others put it inside the ODD.
    Inside,
    /// Outside the ODD whatever the others are.
    Outside,
}

impl From<bool> for State {
    fn from(inside: bool) -> Self {
        if inside {
            State::Inside
        } else {
            State::Outside
        }
    }
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
    pub kind: KindTag,
    pub unit: String,
    pub buckets: Vec<String>,
    /// Each bucket's state, in order.
    pub states: Vec<State>,
    /// Bucket edges of a number, ascending (empty for a categorical).
    pub edges: Vec<f64>,
    /// The values a categorical takes, as labels (empty for a number).
    pub values: Vec<String>,
    /// A number of an integer primitive (`integer`, `long`).
    pub integer: bool,
}

impl Attribute {
    pub fn is_number(&self) -> bool {
        self.kind == KindTag::Number
    }

    pub fn derived(&self) -> bool {
        self.kind == KindTag::Derived
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

/// The ODD read, shared. Two reports of one revision read the same files, so
/// the report's revision decides equality and this always compares equal:
/// re-reading unchanged files changes nothing downstream.
#[derive(Clone, Debug)]
pub struct Shared(pub std::sync::Arc<Odd>);

impl PartialEq for Shared {
    fn eq(&self, _: &Self) -> bool {
        true
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
    /// An attribute by its dotted name, else by its own.
    pub fn attribute(&self, name: &str) -> Option<&Attribute> {
        self.attributes
            .iter()
            .find(|a| a.name == name)
            .or_else(|| self.attributes.iter().find(|a| tail(&a.name) == name))
    }
}

const BINDING: &str = "openodd-editor.binding.yaml";

fn transmission(files: &Files, binding: Option<String>) -> Result<Transmission, String> {
    let mut t = Transmission::default();
    let entry = files.entry();
    t.add(entry, files.text.as_bytes().to_vec(), None)
        .map_err(|e| e.to_string())?;
    for (name, text) in files.library.iter().filter(|(n, _)| n != entry) {
        t.add(name, text.as_bytes().to_vec(), None)
            .map_err(|e| e.to_string())?;
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
/// Read once as it is (nothing measured: each number is read in the unit
/// its conditions write), then with a binding that measures every concept
/// but the derived ones in that unit, so that the reader buckets every
/// attribute and works out which buckets are outside.
pub fn load(files: &Files) -> Result<Loaded, String> {
    let plain =
        load_transmission(&transmission(files, None)?, &AnyProbe).map_err(|e| e.to_string())?;
    let mut probes = serde_norway::Mapping::new();
    for a in plain
        .odd
        .attributes
        .iter()
        .filter(|a| a.concept != ConceptKind::Derived)
    {
        let mut b = serde_norway::Mapping::new();
        b.insert("probe".into(), a.name.clone().into());
        if !a.written_unit.is_empty() {
            b.insert("unit".into(), a.written_unit.clone().into());
        }
        probes.insert(a.name.clone().into(), b.into());
    }
    let mut binding = serde_norway::Mapping::new();
    binding.insert("openodd".into(), vec![files.entry().to_string()].into());
    binding.insert("name".into(), plain.odd.name.clone().into());
    binding.insert("probes".into(), probes.into());
    let binding = serde_norway::to_string(&binding).map_err(|e| e.to_string())?;
    let mut loaded = load_transmission(&transmission(files, Some(binding))?, &AnyProbe)
        .map_err(|e| e.to_string())?;
    // The revision is the document's, not the checker's binding's.
    loaded.transmission = plain.transmission;
    Ok(loaded)
}

fn kind_of(c: &ConceptKind) -> KindTag {
    match c {
        ConceptKind::Number { .. } => KindTag::Number,
        ConceptKind::Boolean => KindTag::Boolean,
        ConceptKind::Categorical => KindTag::Categorical,
        ConceptKind::Derived => KindTag::Derived,
        ConceptKind::Other => KindTag::Other,
    }
}

pub fn check(files: &Files) -> Result<Report, String> {
    let loaded = load(files)?;
    let odd = &loaded.odd;
    let attributes = odd
        .attributes
        .iter()
        .map(|a| {
            let item = a.item.as_ref();
            let buckets = item.map(|i| i.labels.clone()).unwrap_or_default();
            Attribute {
                name: a.name.clone(),
                kind: kind_of(&a.concept),
                unit: a.unit.clone(),
                states: (0..buckets.len())
                    .map(|i| {
                        verdict_at(odd, &[(&a.name, Value::Bucket(i))])
                            .inside
                            .into()
                    })
                    .collect(),
                buckets,
                edges: item.map(|i| i.edges.clone()).unwrap_or_default(),
                values: item
                    .map(|i| i.values.iter().map(Value::label).collect())
                    .unwrap_or_default(),
                integer: matches!(&a.concept, ConceptKind::Number { primitive, .. }
                    if primitive == "integer" || primitive == "long"),
            }
        })
        .collect();
    let modules = odd
        .modules
        .iter()
        .map(|m| {
            let section = |s: &Option<openodd::Section>| {
                s.as_ref()
                    .map(|s| (s.all, s.conditions.iter().map(|c| c.describe()).collect()))
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
                let targets = refs
                    .into_iter()
                    .map(|r| (r, true))
                    .chain(attrs.into_iter().map(|a| (a, false)));
                for (target, module) in targets {
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

/// The ODD's verdict with these attributes at these values and every other
/// one open (anything it may be).
fn verdict_at(odd: &Odd, at: &[(&str, Value)]) -> Verdict {
    let mut values: Values = odd
        .attributes
        .iter()
        .map(|b| (b.name.clone(), Value::Open))
        .collect();
    for (name, v) in at {
        values.insert(name.to_string(), v.clone());
    }
    odd.complete_open(&mut values);
    odd.evaluate(&values)
}

/// Whether the ODD, or one module, can hold in this verdict.
fn holds(v: &Verdict, module: Option<&str>) -> State {
    match module {
        None => v.inside,
        Some(m) => v.modules.get(m) != Some(&Truth::False),
    }
    .into()
}

/// What one module makes of each bucket of an attribute, the others aside:
/// outside when the module cannot hold there.
pub fn module_states(report: &Report, module: &str, attribute: &str) -> Vec<State> {
    let Some(a) = report.attribute(attribute) else {
        return vec![];
    };
    (0..a.buckets.len())
        .map(|i| {
            let v = verdict_at(&report.odd.0, &[(&a.name, Value::Bucket(i))]);
            holds(&v, Some(module))
        })
        .collect()
}

/// Where the ODD (or one module) stands along a numeric attribute between
/// `lo` and `hi`, the others aside, as runs `(from, to, state)`.
///
/// A verdict changes only at a bucket edge (the reader cuts the buckets
/// where conditions draw lines), so it is asked inside each stretch between
/// edges and at each edge: a bound that includes its edge (`<= 2`) moves the
/// change to just past it. For an integer the change falls halfway between
/// whole numbers.
pub fn runs(
    report: &Report,
    attribute: &str,
    module: Option<&str>,
    lo: f64,
    hi: f64,
) -> Vec<(f64, f64, State)> {
    let Some(a) = report.attribute(attribute) else {
        return vec![];
    };
    let at = |x: f64| {
        holds(
            &verdict_at(&report.odd.0, &[(&a.name, Value::Float(x))]),
            module,
        )
    };
    let edges: Vec<f64> = a
        .edges
        .iter()
        .copied()
        .filter(|e| e.is_finite() && *e > lo && *e < hi)
        .collect();
    let bounds: Vec<f64> = std::iter::once(lo)
        .chain(edges.iter().copied())
        .chain(std::iter::once(hi))
        .collect();
    // A value inside each stretch: its middle, or its first whole number.
    let inside: Vec<Option<State>> = bounds
        .windows(2)
        .map(|w| {
            let x = if a.integer {
                let c = w[0].floor() + 1.0;
                (c < w[1]).then_some(c)
            } else {
                Some((w[0] + w[1]) / 2.0)
            };
            x.map(at)
        })
        .collect();
    let half = if a.integer { 0.5 } else { 0.0 };
    let sliver = if a.integer { 0.5 } else { (hi - lo) * 0.004 };
    let mut cuts: Vec<(f64, State)> = vec![];
    let mut first = inside[0];
    for (k, e) in edges.iter().enumerate() {
        let here = at(*e);
        let left = inside[k].unwrap_or(here);
        let right = inside[k + 1].unwrap_or(here);
        first = first.or(Some(left));
        if here == left && here != right {
            cuts.push((e + half, right));
        } else if here == right && here != left {
            cuts.push((e - half, right));
        } else if here != left {
            cuts.push((e - sliver, here));
            cuts.push((e + sliver, right));
        }
    }
    let mut out: Vec<(f64, f64, State)> = vec![];
    let mut start = (lo, first.unwrap_or_else(|| at((lo + hi) / 2.0)));
    for (x, st) in cuts {
        out.push((start.0, x, start.1));
        start = (x, st);
    }
    out.push((start.0, hi, start.1));
    out
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
                .map(|j| {
                    let at = [
                        (ax.name.as_str(), Value::Bucket(j)),
                        (ay.name.as_str(), Value::Bucket(i)),
                    ];
                    verdict_at(&report.odd.0, &at).inside.into()
                })
                .collect()
        })
        .collect()
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
        if text.is_empty() || attr.concept == ConceptKind::Derived {
            continue;
        }
        let number = matches!(attr.concept, ConceptKind::Number { .. });
        let value = match (text, number.then(|| parse_float(text)).flatten()) {
            ("true", _) => Value::Bool(true),
            ("false", _) => Value::Bool(false),
            (_, Some(x)) => Value::Float(x),
            _ => Value::Str(text.to_string()),
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
            let v = match values.get(&a.name) {
                None | Some(Value::Missing) => "unknown".to_string(),
                Some(v) => v.label(),
            };
            (a.name.clone(), v)
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
        assert_eq!(fog.states.last(), Some(&State::Outside));
        let rain = r
            .attributes
            .iter()
            .find(|a| a.name == "environment.rainfall")
            .unwrap();
        assert!(rain.derived());
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
        // `<= 2` on integers: inside up to 2, outside from 3 (cut at 2.5).
        let lanes = runs(&r, "scenery.lane_count", Some("town_roads"), 0.0, 6.0);
        assert_eq!(
            lanes,
            [(0.0, 2.5, State::Inside), (2.5, 6.0, State::Outside)]
        );
        // `[0 .. 40] km/h` on the ODD: outside below 0 and above 40.
        let speed = runs(&r, "dynamic.ego_speed", None, -10.0, 60.0);
        let states: Vec<State> = speed.iter().map(|r| r.2).collect();
        assert_eq!(states, [State::Outside, State::Inside, State::Outside]);
        assert_eq!(speed[0].1, 0.0);
        assert!((speed[2].0 - 40.0).abs() < 0.5, "{speed:?}");
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
        assert_eq!(r.attributes[0].states, [State::Inside, State::Outside]);
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
