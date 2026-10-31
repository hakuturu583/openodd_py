//! A binding file and a backend's manifest, as the editor holds them, and
//! what connects the three things the studio edits.
//!
//! - The **ODD** says what is talked about: `environment.rain` is `none`,
//!   `light`, `moderate` or `heavy`.
//! - The **binding** says how each attribute is measured: by the backend's
//!   probe `rain` ([`BindingDoc`]).
//! - The backend's **manifest** ([`Manifest`], written by Python's
//!   `Backend.manifest()`) says what its probes read in, how its knobs set
//!   each bucket (`heavy`: `environment.rain` from 70 to 100), and which of
//!   its rule inputs a knob writes (`precipitation` is `environment.rain`).
//!
//! [`links`] follows an attribute through all three to the rule inputs that
//! read what it sets: what the studio draws and checks.

use openodd::loader::Resolver;
use serde_norway::{Mapping, Value};

use crate::doc::fresh_id;

// ---------------------------------------------------------------------------
// The manifest
// ---------------------------------------------------------------------------

/// What a knob writes for one bucket label.
#[derive(Clone, Debug, PartialEq)]
pub enum KnobValue {
    Point(f64),
    Range(f64, f64),
    Text(String),
}

impl KnobValue {
    /// The span it writes, for a number.
    pub fn span(&self) -> Option<(f64, f64)> {
        match *self {
            KnobValue::Point(v) => Some((v, v)),
            KnobValue::Range(a, b) => Some((a, b)),
            KnobValue::Text(_) => None,
        }
    }

    pub fn words(&self) -> String {
        match self {
            KnobValue::Point(v) => crate::rules_doc::num(*v),
            KnobValue::Range(a, b) => {
                format!(
                    "{} – {}",
                    crate::rules_doc::num(*a),
                    crate::rules_doc::num(*b)
                )
            }
            KnobValue::Text(t) => t.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProbeInfo {
    pub name: String,
    pub unit: String,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct KnobInfo {
    /// The probe whose attribute it sets.
    pub probe: String,
    /// The setting it writes.
    pub key: String,
    pub values: Vec<(String, KnobValue)>,
    pub range: Option<(f64, f64)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RuleInputInfo {
    pub name: String,
    /// The key of the knob whose written value it is.
    pub knob: Option<String>,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Manifest {
    pub name: String,
    pub probes: Vec<ProbeInfo>,
    pub readers: Vec<String>,
    pub knobs: Vec<KnobInfo>,
    pub rule_inputs: Vec<RuleInputInfo>,
    /// The rule files the backend follows: (file, sha256).
    pub rules: Vec<(String, String)>,
}

impl Manifest {
    /// A manifest, from its YAML or JSON.
    pub fn from_text(text: &str) -> Result<Self, String> {
        let v: Value = serde_norway::from_str(text).map_err(|e| e.to_string())?;
        let Value::Mapping(top) = v else {
            return Err("a manifest is a mapping".into());
        };
        if top.get("openodd_backend").and_then(Value::as_i64) != Some(1) {
            return Err("not a backend manifest (openodd_backend: 1)".into());
        }
        let mut m = Manifest {
            name: top.get("name").map(text_of).unwrap_or_default(),
            ..Default::default()
        };
        for (k, v) in mapping(top.get("probes")) {
            m.probes.push(ProbeInfo {
                name: text_of(&k),
                unit: v.get("unit").map(text_of).unwrap_or_default(),
                text: v.get("text").map(text_of).unwrap_or_default(),
            });
        }
        if let Some(Value::Sequence(rs)) = top.get("readers") {
            m.readers = rs.iter().map(text_of).collect();
        }
        for (k, v) in mapping(top.get("knobs")) {
            let values = mapping(v.get("values"))
                .iter()
                .map(|(l, x)| (text_of(l), knob_value(x)))
                .collect();
            m.knobs.push(KnobInfo {
                probe: text_of(&k),
                key: v.get("key").map(text_of).unwrap_or_default(),
                values,
                range: v.get("range").and_then(pair),
            });
        }
        for (k, v) in mapping(top.get("rule_inputs")) {
            m.rule_inputs.push(RuleInputInfo {
                name: text_of(&k),
                knob: v.get("knob").map(text_of),
                text: v.get("text").map(text_of).unwrap_or_default(),
            });
        }
        if let Some(Value::Sequence(rs)) = top.get("rules") {
            for r in rs {
                m.rules.push((
                    r.get("file").map(text_of).unwrap_or_default(),
                    r.get("sha256").map(text_of).unwrap_or_default(),
                ));
            }
        }
        Ok(m)
    }

    pub fn probe(&self, name: &str) -> Option<&ProbeInfo> {
        self.probes.iter().find(|p| p.name == name)
    }

    pub fn knob(&self, probe: &str) -> Option<&KnobInfo> {
        self.knobs.iter().find(|k| k.probe == probe)
    }

    pub fn rule_input(&self, name: &str) -> Option<&RuleInputInfo> {
        self.rule_inputs.iter().find(|r| r.name == name)
    }
}

/// The manifest as the reader's resolver: its probes and readers are the
/// only ones a binding may name.
pub struct ManifestResolver<'a>(pub &'a Manifest);

impl Resolver for ManifestResolver<'_> {
    fn reader_keys(&self) -> Vec<String> {
        self.0.readers.clone()
    }
    fn probe_unit(&self, spec: &str) -> Result<String, String> {
        self.0.probe(spec).map(|p| p.unit.clone()).ok_or_else(|| {
            let known: Vec<&str> = self.0.probes.iter().map(|p| p.name.as_str()).collect();
            format!(
                "unknown probe '{spec}'; the {} backend has: {}",
                self.0.name,
                known.join(", ")
            )
        })
    }
    fn reader_unit(&self, key: &str, _value: &str) -> Result<String, String> {
        if self.0.readers.iter().any(|r| r == key) {
            Ok(String::new())
        } else {
            Err(format!(
                "the {} backend has no '{key}' readers",
                self.0.name
            ))
        }
    }
}

fn knob_value(v: &Value) -> KnobValue {
    if let Some((a, b)) = pair(v) {
        return KnobValue::Range(a, b);
    }
    match v.as_f64() {
        Some(x) => KnobValue::Point(x),
        None => KnobValue::Text(text_of(v)),
    }
}

fn pair(v: &Value) -> Option<(f64, f64)> {
    match v {
        Value::Sequence(s) if s.len() == 2 => Some((s[0].as_f64()?, s[1].as_f64()?)),
        _ => None,
    }
}

fn mapping(v: Option<&Value>) -> Vec<(Value, Value)> {
    match v {
        Some(Value::Mapping(m)) => m.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
        _ => vec![],
    }
}

fn text_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => String::new(),
        other => serde_norway::to_string(other)
            .unwrap_or_default()
            .trim()
            .to_string(),
    }
}

// ---------------------------------------------------------------------------
// The binding
// ---------------------------------------------------------------------------

/// What measures an attribute.
#[derive(Clone, Debug, PartialEq)]
pub enum Source {
    /// `probe: name` (or `package.module:function`).
    Probe(String),
    /// `measure: value`, or another reader's key.
    Reader(String, String),
}

/// How one attribute is measured.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub id: u64,
    pub attribute: String,
    pub source: Source,
    /// The unit the probe's values are given in, when the backend's is not.
    pub unit: Option<String>,
    /// The values a categorical probe returns, as the file writes them.
    pub values: Option<Vec<Value>>,
    /// The edges a numeric attribute is bucketed at.
    pub buckets: Option<Vec<f64>>,
    pub cover_by: Option<String>,
    pub min_stay: Option<f64>,
    pub text: String,
    /// Other fields (`range`, `every`, `target`), kept as written.
    pub extra: Mapping,
}

impl Entry {
    pub fn new(attribute: String, probe: String) -> Self {
        Entry {
            id: fresh_id(),
            attribute,
            source: Source::Probe(probe),
            unit: None,
            values: None,
            buckets: None,
            cover_by: None,
            min_stay: None,
            text: String::new(),
            extra: Mapping::new(),
        }
    }

    /// The probe it names, if it names one.
    pub fn probe(&self) -> Option<&str> {
        match &self.source {
            Source::Probe(p) => Some(p),
            Source::Reader(..) => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct BindingDoc {
    /// The OpenODD files it measures.
    pub openodd: Vec<String>,
    pub name: String,
    pub text: String,
    pub entries: Vec<Entry>,
    /// Other top-level fields (`situations`), kept as written.
    pub extra: Mapping,
}

impl BindingDoc {
    pub fn entry(&self, attribute: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.attribute == attribute)
    }

    pub fn entry_mut(&mut self, id: u64) -> Option<&mut Entry> {
        self.entries.iter_mut().find(|e| e.id == id)
    }

    pub fn from_yaml(text: &str) -> Result<Self, String> {
        if text.trim().is_empty() {
            return Ok(BindingDoc::default());
        }
        let v: Value = serde_norway::from_str(text).map_err(|e| e.to_string())?;
        let Value::Mapping(top) = v else {
            return Err("a binding file is a mapping: openodd, name, probes".into());
        };
        let mut doc = BindingDoc::default();
        for (k, v) in top {
            match text_of(&k).as_str() {
                "openodd" => {
                    doc.openodd = match &v {
                        Value::Sequence(s) => s.iter().map(text_of).collect(),
                        other => vec![text_of(other)],
                    }
                }
                "name" => doc.name = text_of(&v),
                "text" => doc.text = text_of(&v),
                "probes" => {
                    let Value::Mapping(ps) = v else {
                        return Err("probes: a mapping of attribute to how it is measured".into());
                    };
                    for (a, e) in ps {
                        doc.entries.push(read_entry(&text_of(&a), &e)?);
                    }
                }
                _ => {
                    doc.extra.insert(k, v);
                }
            }
        }
        Ok(doc)
    }

    pub fn to_yaml(&self) -> Result<String, String> {
        let mut top = Mapping::new();
        top.insert(
            "openodd".into(),
            Value::Sequence(self.openodd.iter().map(|s| s.clone().into()).collect()),
        );
        if !self.name.is_empty() {
            top.insert("name".into(), self.name.clone().into());
        }
        if !self.text.is_empty() {
            top.insert("text".into(), self.text.clone().into());
        }
        let mut probes = Mapping::new();
        for e in &self.entries {
            let mut m = Mapping::new();
            match &e.source {
                Source::Probe(p) => m.insert("probe".into(), p.clone().into()),
                Source::Reader(k, v) => m.insert(k.clone().into(), v.clone().into()),
            };
            if let Some(u) = &e.unit {
                m.insert("unit".into(), u.clone().into());
            }
            if let Some(vs) = &e.values {
                m.insert("values".into(), Value::Sequence(vs.clone()));
            }
            if let Some(bs) = &e.buckets {
                m.insert(
                    "buckets".into(),
                    Value::Sequence(bs.iter().map(|b| number(*b)).collect()),
                );
            }
            if let Some(c) = &e.cover_by {
                m.insert("cover_by".into(), c.clone().into());
            }
            if let Some(s) = e.min_stay {
                m.insert("min_stay".into(), number(s));
            }
            for (k, v) in &e.extra {
                m.insert(k.clone(), v.clone());
            }
            if !e.text.is_empty() {
                m.insert("text".into(), e.text.clone().into());
            }
            probes.insert(e.attribute.clone().into(), Value::Mapping(m));
        }
        top.insert("probes".into(), Value::Mapping(probes));
        for (k, v) in &self.extra {
            top.insert(k.clone(), v.clone());
        }
        serde_norway::to_string(&Value::Mapping(top)).map_err(|e| e.to_string())
    }
}

fn number(v: f64) -> Value {
    if v == v.trunc() && v.abs() < 1e15 {
        Value::Number((v as i64).into())
    } else {
        Value::Number(v.into())
    }
}

fn read_entry(attribute: &str, v: &Value) -> Result<Entry, String> {
    let at = format!("probes.{attribute}");
    let mut e = Entry::new(attribute.to_string(), String::new());
    let Value::Mapping(m) = v else {
        // `attribute: probe_name`, the short form.
        e.source = Source::Probe(text_of(v));
        return Ok(e);
    };
    let mut source = None;
    for (k, v) in m {
        match text_of(k).as_str() {
            "probe" => source = Some(Source::Probe(text_of(v))),
            "unit" => e.unit = Some(text_of(v)),
            "values" => match v {
                Value::Sequence(s) => e.values = Some(s.clone()),
                _ => return Err(format!("{at}.values: a list")),
            },
            "buckets" => {
                e.buckets = Some(match v {
                    Value::Sequence(s) => s
                        .iter()
                        .map(|x| x.as_f64().ok_or_else(|| format!("{at}.buckets: numbers")))
                        .collect::<Result<_, _>>()?,
                    _ => return Err(format!("{at}.buckets: a list of edges")),
                })
            }
            "cover_by" => e.cover_by = Some(text_of(v)),
            "min_stay" => e.min_stay = v.as_f64(),
            "text" => e.text = text_of(v),
            "range" | "every" | "target" => {
                e.extra.insert(k.clone(), v.clone());
            }
            other => {
                // Any other key names one of the backend's readers.
                if source.is_some() {
                    return Err(format!("{at}: '{other}' besides a probe"));
                }
                source = Some(Source::Reader(other.to_string(), text_of(v)));
            }
        }
    }
    e.source = source.ok_or_else(|| format!("{at}: no probe"))?;
    Ok(e)
}

// ---------------------------------------------------------------------------
// Checking
// ---------------------------------------------------------------------------

/// openodd's reading of the binding `text` (named `name`) over the ODD
/// `odd`, its probes resolved by `manifest` (any name, without one).
pub fn check(
    odd: &crate::check::Files,
    name: &str,
    text: &str,
    manifest: Option<&Manifest>,
) -> Result<openodd::Loaded, String> {
    let entry = if odd.entry.is_empty() {
        "odd.yaml"
    } else {
        odd.entry.as_str()
    };
    if name == entry {
        return Err(format!("the binding and the ODD are both named {name}"));
    }
    let mut t = openodd::Transmission::default();
    let mut add = |n: &str, s: &str| {
        t.add(n, s.as_bytes().to_vec(), None)
            .map_err(|e| e.to_string())
    };
    add(entry, &odd.text)?;
    for (n, s) in odd.library.iter().filter(|(n, _)| n != entry && n != name) {
        add(n, s)?;
    }
    add(name, text)?;
    t.entry = name.to_string();
    let loaded = match manifest {
        Some(m) => openodd::load_transmission(&t, &ManifestResolver(m)),
        None => openodd::load_transmission(&t, &openodd::loader::AnyProbe),
    };
    loaded.map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// The connections
// ---------------------------------------------------------------------------

/// An attribute followed to the rules: what measures it, how a run sets each
/// of its buckets, and the rule inputs that read what it sets.
#[derive(Clone, Debug, PartialEq)]
pub struct Link {
    pub attribute: String,
    /// Its buckets, as the ODD names them.
    pub labels: Vec<String>,
    pub probe: Option<String>,
    pub knob: Option<KnobInfo>,
    /// The rule inputs the knob writes.
    pub inputs: Vec<String>,
}

impl Link {
    /// Each bucket's span in the knob's values: what the rule input reads
    /// when the ODD says that label.
    pub fn spans(&self) -> Vec<(String, f64, f64)> {
        let Some(k) = &self.knob else { return vec![] };
        self.labels
            .iter()
            .filter_map(|l| {
                let (_, v) = k.values.iter().find(|(n, _)| n == l)?;
                let (a, b) = v.span()?;
                Some((l.clone(), a, b))
            })
            .collect()
    }
}

/// Every bound attribute of `attributes` (name, bucket labels), followed
/// through `binding` and `manifest`.
pub fn links(
    attributes: &[(String, Vec<String>)],
    binding: &BindingDoc,
    manifest: Option<&Manifest>,
) -> Vec<Link> {
    attributes
        .iter()
        .filter_map(|(name, labels)| {
            let e = binding.entry(name)?;
            let probe = e.probe().map(str::to_string);
            let knob = probe
                .as_deref()
                .and_then(|p| manifest.and_then(|m| m.knob(p)))
                .cloned();
            let inputs = match (&knob, manifest) {
                (Some(k), Some(m)) => m
                    .rule_inputs
                    .iter()
                    .filter(|r| r.knob.as_deref() == Some(k.key.as_str()))
                    .map(|r| r.name.clone())
                    .collect(),
                _ => vec![],
            };
            Some(Link {
                attribute: name.clone(),
                labels: labels.clone(),
                probe,
                knob,
                inputs,
            })
        })
        .collect()
}

/// How a rule input's terms meet the ODD's buckets: per bucket, the term
/// strongest at its middle, and whether its name is the bucket's.
pub fn alignment(
    spans: &[(String, f64, f64)],
    terms: &[(String, crate::rules_doc::Shape)],
) -> Vec<(String, Option<String>, bool)> {
    spans
        .iter()
        .map(|(label, a, b)| {
            let mid = (a + b) / 2.0;
            let best = terms
                .iter()
                .map(|(n, s)| (n, s.degree(mid)))
                .filter(|(_, d)| *d > 0.0)
                .max_by(|x, y| x.1.total_cmp(&y.1))
                .map(|(n, _)| n.clone());
            let same = best.as_deref() == Some(label.as_str());
            (label.clone(), best, same)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules_doc::Shape;

    const MANIFEST: &str = include_str!("../examples/carla.manifest.yaml");
    const BINDING: &str = include_str!("../examples/carla_weather.binding.yaml");

    #[test]
    fn a_manifest_reads_probes_knobs_and_rule_inputs() {
        let m = Manifest::from_text(MANIFEST).unwrap();
        assert_eq!(m.name, "CARLA");
        let rain = m.knob("rain").unwrap();
        assert_eq!(rain.key, "environment.rain");
        assert_eq!(
            rain.values[3],
            ("heavy".to_string(), KnobValue::Range(70.0, 100.0))
        );
        assert_eq!(rain.values[0].1, KnobValue::Point(0.0));
        assert_eq!(
            m.rule_input("precipitation").unwrap().knob.as_deref(),
            Some("environment.rain")
        );
        assert!(ManifestResolver(&m).probe_unit("rain").is_ok());
        assert!(ManifestResolver(&m)
            .probe_unit("snow")
            .unwrap_err()
            .contains("unknown probe 'snow'"));
    }

    #[test]
    fn a_binding_reads_and_writes_back() {
        let b = BindingDoc::from_yaml(BINDING).unwrap();
        assert_eq!(b.openodd, ["weather.yaml"]);
        assert_eq!(b.entry("environment.rain").unwrap().probe(), Some("rain"));
        let again = BindingDoc::from_yaml(&b.to_yaml().unwrap()).unwrap();
        let strip = |d: &BindingDoc| {
            d.entries
                .iter()
                .map(|e| {
                    (
                        e.attribute.clone(),
                        e.source.clone(),
                        e.buckets.clone(),
                        e.text.clone(),
                    )
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(strip(&again), strip(&b));
        let short = BindingDoc::from_yaml(
            "openodd: [a.yaml]\nprobes:\n  x: speed\n  gap: {measure: gap_m, buckets: [0, 10]}\n",
        )
        .unwrap();
        assert_eq!(short.entries[0].source, Source::Probe("speed".into()));
        assert_eq!(
            short.entries[1].source,
            Source::Reader("measure".into(), "gap_m".into())
        );
    }

    #[test]
    fn a_binding_is_read_with_its_odd_and_the_backends_probes() {
        let odd = crate::check::Files {
            entry: "weather.yaml".into(),
            text: include_str!("../examples/weather.yaml").into(),
            library: vec![],
        };
        let m = Manifest::from_text(MANIFEST).unwrap();
        let loaded = check(&odd, "binding.yaml", BINDING, Some(&m)).unwrap();
        let rain = loaded
            .odd
            .attributes
            .iter()
            .find(|a| a.name == "environment.rain")
            .unwrap();
        assert_eq!(
            rain.item.as_ref().unwrap().labels,
            ["none", "light", "moderate", "heavy"]
        );
        let wrong = BINDING.replace("probe: fog", "probe: mist");
        assert!(check(&odd, "binding.yaml", &wrong, Some(&m))
            .unwrap_err()
            .contains("unknown probe 'mist'"));
        // Without a manifest any probe name reads.
        assert!(check(&odd, "binding.yaml", &wrong, None).is_ok());
        // An attribute left out is read, unmeasured.
        let part = "openodd: [weather.yaml]\nprobes:\n  environment.rain: {probe: rain}\n";
        assert!(check(&odd, "binding.yaml", part, Some(&m)).is_ok());
    }

    #[test]
    fn an_attribute_is_followed_to_the_rule_inputs_its_knob_writes() {
        let m = Manifest::from_text(MANIFEST).unwrap();
        let b = BindingDoc::from_yaml(BINDING).unwrap();
        let labels = |xs: &[&str]| xs.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let attrs = vec![
            (
                "environment.rain".to_string(),
                labels(&["none", "light", "moderate", "heavy"]),
            ),
            ("environment.ghost".to_string(), labels(&["a"])),
        ];
        let ls = links(&attrs, &b, Some(&m));
        assert_eq!(ls.len(), 1);
        assert_eq!(ls[0].inputs, ["precipitation"]);
        assert_eq!(ls[0].spans()[3], ("heavy".into(), 70.0, 100.0));
        // The sample rules' rain terms peak where the ODD's buckets end.
        let terms = vec![
            ("dry".to_string(), Shape::Trapezoid([0.0, 0.0, 0.0, 30.0])),
            ("light".to_string(), Shape::Triangle([0.0, 30.0, 70.0])),
            ("heavy".to_string(), Shape::Triangle([30.0, 70.0, 100.0])),
            (
                "torrential".to_string(),
                Shape::Trapezoid([70.0, 100.0, 100.0, 100.0]),
            ),
        ];
        let a = alignment(&ls[0].spans(), &terms);
        assert_eq!(a[0], ("none".into(), Some("dry".into()), false));
        assert_eq!(a[1], ("light".into(), Some("light".into()), true));
    }
}
