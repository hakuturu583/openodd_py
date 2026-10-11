//! The Python extension `openodd._core`.
//!
//! Thin: the Python package (`python/openodd`) holds the probes -- Python
//! callables -- and hands this module plain values. Structured results that
//! Python only reads (definitions, manifests) cross as JSON text.

use std::collections::HashMap;
use std::path::PathBuf;

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict, PyList};

use crate::error::Error;
use crate::items::{Buckets, CoverGroup, CoverItem as CoreItem, Criteria};
use crate::loader::{self, Loaded, Options, Resolver, Source};
use crate::model::{
    self, Attribute as CoreAttribute, Condition as CoreCondition, Module as CoreModule, Predicate,
    Probe, Section, Truth,
};
use crate::sources::GitSource;
use crate::transmission::Transmission;
use crate::value::Value;
use crate::yaml::{parse_all, Node};

pyo3::create_exception!(
    _core,
    OpenOddError,
    PyValueError,
    "An OpenODD document this reader cannot make an ODD of."
);
pyo3::create_exception!(
    _core,
    UnitError,
    PyValueError,
    "A unit that is unknown, or that cannot be converted."
);
pyo3::create_exception!(
    _core,
    GitSourceError,
    PyValueError,
    "A git source could not be fetched."
);

fn err(e: Error) -> PyErr {
    match e {
        Error::OpenOdd(m) => OpenOddError::new_err(m),
        Error::Unit(m) => UnitError::new_err(m),
        Error::GitSource(m) => GitSourceError::new_err(m),
        Error::Value(m) | Error::Io(m) => PyValueError::new_err(m),
    }
}

/// An attribute whose value is open: present, but could be anything.
#[pyclass(frozen, module = "openodd._core")]
struct Undecided;

#[pymethods]
impl Undecided {
    fn __repr__(&self) -> &'static str {
        "UNDECIDED"
    }
    fn __reduce__(&self) -> &'static str {
        "UNDECIDED"
    }
}

fn to_value(obj: &Bound<'_, PyAny>) -> PyResult<Value> {
    if obj.is_none() {
        return Ok(Value::Missing);
    }
    if obj.is_instance_of::<Undecided>() {
        return Ok(Value::Open);
    }
    if let Ok(b) = obj.downcast::<pyo3::types::PyBool>() {
        return Ok(Value::Bool(b.is_true()));
    }
    if obj.is_instance_of::<pyo3::types::PyInt>() {
        return Ok(match obj.extract::<i64>() {
            Ok(i) => Value::Int(i),
            Err(_) => Value::Float(obj.extract::<f64>()?),
        });
    }
    if obj.is_instance_of::<pyo3::types::PyFloat>() {
        return Ok(Value::Float(obj.extract::<f64>()?));
    }
    Ok(Value::Str(obj.str()?.to_string()))
}

fn from_value(py: Python<'_>, v: &Value) -> PyResult<PyObject> {
    Ok(match v {
        Value::Missing => py.None(),
        Value::Bool(b) => b.into_pyobject(py)?.to_owned().into_any().unbind(),
        Value::Int(i) => i.into_pyobject(py)?.into_any().unbind(),
        Value::Float(x) => x.into_pyobject(py)?.into_any().unbind(),
        Value::Str(s) => s.into_pyobject(py)?.into_any().unbind(),
        Value::Open => Py::new(py, Undecided)?.into_any(),
        Value::Bucket(i) => format!("<bucket {i}>")
            .into_pyobject(py)?
            .into_any()
            .unbind(),
    })
}

fn to_values(values: &Bound<'_, PyDict>) -> PyResult<model::Values> {
    let mut out = HashMap::with_capacity(values.len());
    for (k, v) in values.iter() {
        out.insert(k.str()?.to_string(), to_value(&v)?);
    }
    Ok(out)
}

fn truth(py: Python<'_>, t: Truth) -> PyResult<PyObject> {
    Ok(match t {
        Truth::True => true.into_pyobject(py)?.to_owned().into_any().unbind(),
        Truth::False => false.into_pyobject(py)?.to_owned().into_any().unbind(),
        Truth::Unknown => py.None(),
        Truth::Inactive => "inactive".into_pyobject(py)?.into_any().unbind(),
    })
}

// ---------------------------------------------------------------------------
// Cover items
// ---------------------------------------------------------------------------

/// The buckets of a cover item.
#[pyclass(frozen, module = "openodd._core")]
#[derive(Clone)]
struct Bucketing {
    inner: CoreItem,
}

#[pymethods]
impl Bucketing {
    /// Buckets for `values` (one each), `buckets` (edges) or `range` + `every`.
    #[new]
    #[pyo3(signature = (name, *, values=None, buckets=None, range=None, every=None, event="end", target=1.0, cover_by="hits", min_stay=None))]
    #[allow(clippy::too_many_arguments)]
    fn new(
        name: &str,
        values: Option<Vec<Bound<'_, PyAny>>>,
        buckets: Option<Vec<f64>>,
        range: Option<(f64, f64)>,
        every: Option<f64>,
        event: &str,
        target: f64,
        cover_by: &str,
        min_stay: Option<f64>,
    ) -> PyResult<Self> {
        let mut given = Vec::new();
        if values.is_some() {
            given.push("values");
        }
        if buckets.is_some() {
            given.push("buckets");
        }
        if range.is_some() {
            given.push("range");
        }
        if given.len() != 1 {
            return Err(PyValueError::new_err(format!(
                "cover({name}): give exactly one of values, buckets or range (with every); got {}",
                if given.is_empty() {
                    "none".to_string()
                } else {
                    format!("{given:?}").replace('"', "'")
                }
            )));
        }
        if every.is_some() && range.is_none() {
            return Err(PyValueError::new_err(format!(
                "cover({name}): every needs a range"
            )));
        }
        let b = if let Some(values) = values {
            Buckets::Values(values.iter().map(to_value).collect::<PyResult<_>>()?)
        } else if let Some(edges) = buckets {
            Buckets::Edges(edges)
        } else {
            let (low, high) = range.unwrap();
            let every = every.ok_or_else(|| {
                PyValueError::new_err(format!("cover({name}): range needs every"))
            })?;
            Buckets::Range { low, high, every }
        };
        let criteria = Criteria {
            target,
            cover_by: cover_by.into(),
            min_stay,
        };
        let inner = CoreItem::new(name, b, event, CoverGroup::Scenario, criteria).map_err(err)?;
        Ok(Bucketing { inner })
    }

    #[getter]
    fn labels(&self) -> Vec<String> {
        self.inner.labels.clone()
    }
    #[getter]
    fn edges(&self) -> Vec<f64> {
        self.inner.edges.clone()
    }
    #[getter]
    fn values(&self, py: Python<'_>) -> PyResult<Vec<PyObject>> {
        self.inner
            .values
            .iter()
            .map(|v| from_value(py, v))
            .collect()
    }
    #[getter]
    fn numeric(&self) -> bool {
        self.inner.numeric()
    }
    #[getter]
    fn name(&self) -> String {
        self.inner.name.clone()
    }
    #[getter]
    fn target(&self) -> f64 {
        self.inner.criteria.target
    }
    #[getter]
    fn cover_by(&self) -> String {
        self.inner.criteria.cover_by.clone()
    }
    #[getter]
    fn min_stay(&self) -> Option<f64> {
        self.inner.criteria.min_stay
    }
    #[getter]
    fn unit(&self) -> String {
        self.inner.unit.clone()
    }
    #[getter]
    fn text(&self) -> String {
        self.inner.text.clone()
    }
    #[getter]
    fn group(&self) -> &'static str {
        self.inner.group.as_str()
    }

    /// The label of the bucket `value` falls in.
    fn bucket_of(&self, value: &Bound<'_, PyAny>) -> PyResult<String> {
        self.inner.bucket_of(&to_value(value)?).map_err(err)
    }
}

// ---------------------------------------------------------------------------
// Conditions, attributes, modules
// ---------------------------------------------------------------------------

/// A condition of a module.
#[pyclass(frozen, module = "openodd._core")]
#[derive(Clone)]
struct Condition {
    inner: CoreCondition,
}

fn conditions(items: Option<Vec<Py<Condition>>>, py: Python<'_>) -> Vec<CoreCondition> {
    items
        .unwrap_or_default()
        .iter()
        .map(|c| c.borrow(py).inner.clone())
        .collect()
}

#[pymethods]
impl Condition {
    fn describe(&self) -> String {
        self.inner.describe()
    }
    fn __repr__(&self) -> String {
        format!("<OddCondition {}>", self.inner.describe())
    }
    /// The attributes it tests.
    fn attributes(&self) -> Vec<String> {
        let mut out = Vec::new();
        self.inner.attributes(&mut out);
        out
    }
    /// The modules and labels it refers to.
    fn references(&self) -> Vec<String> {
        let mut out = std::collections::BTreeSet::new();
        self.inner.references(&mut out);
        out.into_iter().collect()
    }
}

#[pyfunction]
fn is_in(attribute: String, values: Vec<Bound<'_, PyAny>>) -> PyResult<Condition> {
    if values.is_empty() {
        return Err(PyValueError::new_err(format!(
            "{attribute}.is_in(): no values"
        )));
    }
    let values: Vec<Value> = values.iter().map(to_value).collect::<PyResult<_>>()?;
    let numbers = values.iter().filter_map(Value::number).collect();
    let labels = values.iter().map(Value::label).collect();
    Ok(Condition {
        inner: CoreCondition::Leaf {
            attribute,
            predicate: Predicate::InSet { labels, numbers },
        },
    })
}

#[pyfunction]
fn equals(attribute: String, value: &Bound<'_, PyAny>) -> PyResult<Condition> {
    Ok(Condition {
        inner: CoreCondition::Leaf {
            attribute,
            predicate: Predicate::Equal(to_value(value)?),
        },
    })
}

#[pyfunction]
#[pyo3(signature = (attribute, low=f64::NEG_INFINITY, high=f64::INFINITY, include_low=true, include_high=true))]
fn interval(
    attribute: String,
    low: f64,
    high: f64,
    include_low: bool,
    include_high: bool,
) -> Condition {
    Condition {
        inner: CoreCondition::Leaf {
            attribute,
            predicate: Predicate::Interval {
                low,
                high,
                include_low,
                include_high,
            },
        },
    }
}

#[pyfunction]
fn is_unknown(attribute: String) -> Condition {
    Condition {
        inner: CoreCondition::Missing { attribute },
    }
}

#[pyfunction]
fn all_of(py: Python<'_>, children: Vec<Py<Condition>>) -> PyResult<Condition> {
    Ok(Condition {
        inner: CoreCondition::all_of(conditions(Some(children), py)).map_err(err)?,
    })
}

#[pyfunction]
fn any_of(py: Python<'_>, children: Vec<Py<Condition>>) -> PyResult<Condition> {
    Ok(Condition {
        inner: CoreCondition::any_of(conditions(Some(children), py)).map_err(err)?,
    })
}

#[pyfunction]
#[pyo3(signature = (name, holds=true))]
fn module_holds(name: String, holds: bool) -> PyResult<Condition> {
    Ok(Condition {
        inner: CoreCondition::module_holds(name, holds).map_err(err)?,
    })
}

/// A measurable taxonomy concept.
#[pyclass(module = "openodd._core")]
#[derive(Clone)]
struct Attribute {
    inner: CoreAttribute,
}

#[pymethods]
impl Attribute {
    /// `probe`: the backend key the value is read by (`None`: always missing).
    #[new]
    #[pyo3(signature = (name, probe=None, *, unit="", text="", values=None, buckets=None, range=None, every=None, target=1.0, cover_by="hits", min_stay=None))]
    #[allow(clippy::too_many_arguments)]
    fn new(
        name: String,
        probe: Option<String>,
        unit: &str,
        text: &str,
        values: Option<Vec<Bound<'_, PyAny>>>,
        buckets: Option<Vec<f64>>,
        range: Option<(f64, f64)>,
        every: Option<f64>,
        target: f64,
        cover_by: &str,
        min_stay: Option<f64>,
    ) -> PyResult<Self> {
        let probe = match probe {
            Some(spec) => Probe::External {
                kind: "code".into(),
                spec,
            },
            None => Probe::Missing,
        };
        let criteria = Criteria {
            target,
            cover_by: cover_by.into(),
            min_stay,
        };
        let mut inner =
            CoreAttribute::new(name.clone(), probe, unit, text, criteria).map_err(err)?;
        let given = values.is_some() as u8 + buckets.is_some() as u8 + range.is_some() as u8;
        if given == 0 {
            if every.is_some() {
                return Err(PyValueError::new_err(format!(
                    "OddAttribute({name}): every needs a range"
                )));
            }
        } else {
            if given > 1 {
                return Err(PyValueError::new_err(format!(
                    "cover(odd.{name}): give exactly one of values, buckets or range (with every)"
                )));
            }
            let b = if let Some(values) = values {
                Buckets::Values(values.iter().map(to_value).collect::<PyResult<_>>()?)
            } else if let Some(edges) = buckets {
                Buckets::Edges(edges)
            } else {
                let (low, high) = range.unwrap();
                let every = every.ok_or_else(|| {
                    PyValueError::new_err(format!("cover(odd.{name}): range needs every"))
                })?;
                Buckets::Range { low, high, every }
            };
            inner.set_buckets(b).map_err(err)?;
        }
        Ok(Attribute { inner })
    }

    #[getter]
    fn name(&self) -> String {
        self.inner.name.clone()
    }
    #[getter]
    fn unit(&self) -> String {
        self.inner.unit.clone()
    }
    #[getter]
    fn text(&self) -> String {
        self.inner.text.clone()
    }
    #[getter]
    fn target(&self) -> f64 {
        self.inner.criteria.target
    }
    #[getter]
    fn cover_by(&self) -> String {
        self.inner.criteria.cover_by.clone()
    }
    #[getter]
    fn min_stay(&self) -> Option<f64> {
        self.inner.criteria.min_stay
    }
    /// `(kind, spec)` of the probe: `("probe", "speed_kph")`, `("measure", ...)`,
    /// `("code", key)`, `("derived", "")` or `("missing", "")`.
    #[getter]
    fn probe(&self) -> (String, String) {
        match &self.inner.probe {
            Probe::Missing => ("missing".into(), String::new()),
            Probe::External { kind, spec } => (kind.clone(), spec.clone()),
            Probe::Derived(_) => ("derived".into(), String::new()),
        }
    }
    #[getter]
    fn item(&self) -> Option<Bucketing> {
        self.inner.item.clone().map(|inner| Bucketing { inner })
    }
}

/// A named rule of an ODD.
#[pyclass(module = "openodd._core")]
#[derive(Clone)]
struct Module {
    inner: CoreModule,
}

#[pymethods]
impl Module {
    #[new]
    #[pyo3(signature = (name, *, include_and=None, include_or=None, exclude_and=None, exclude_or=None, labels=None, active=true, text="", situation=false, target=1.0, cover_by="hits", min_stay=None, metadata=None))]
    #[allow(clippy::too_many_arguments)]
    fn new(
        py: Python<'_>,
        name: String,
        include_and: Option<Vec<Py<Condition>>>,
        include_or: Option<Vec<Py<Condition>>>,
        exclude_and: Option<Vec<Py<Condition>>>,
        exclude_or: Option<Vec<Py<Condition>>>,
        labels: Option<Vec<String>>,
        active: bool,
        text: &str,
        situation: bool,
        target: f64,
        cover_by: &str,
        min_stay: Option<f64>,
        metadata: Option<&str>,
    ) -> PyResult<Self> {
        let (ia, io, ea, eo) = (
            conditions(include_and, py),
            conditions(include_or, py),
            conditions(exclude_and, py),
            conditions(exclude_or, py),
        );
        if !ia.is_empty() && !io.is_empty() {
            return Err(PyValueError::new_err(format!(
                "OddModule({name}): one include section, INCLUDE_AND or INCLUDE_OR"
            )));
        }
        if !ea.is_empty() && !eo.is_empty() {
            return Err(PyValueError::new_err(format!(
                "OddModule({name}): one exclude section, EXCLUDE_AND or EXCLUDE_OR"
            )));
        }
        let include = if !ia.is_empty() {
            Some(Section {
                all: true,
                conditions: ia,
            })
        } else if !io.is_empty() {
            Some(Section {
                all: false,
                conditions: io,
            })
        } else {
            None
        };
        let exclude = if !ea.is_empty() {
            Some(Section {
                all: true,
                conditions: ea,
            })
        } else if !eo.is_empty() {
            Some(Section {
                all: false,
                conditions: eo,
            })
        } else {
            None
        };
        let mut inner = CoreModule::new(name.clone(), include, exclude).map_err(err)?;
        inner.labels = labels.unwrap_or_default();
        inner.active = active;
        inner.text = text.into();
        if let Some(m) = metadata {
            inner.metadata =
                serde_json::from_str(m).map_err(|e| PyValueError::new_err(e.to_string()))?;
        }
        let criteria = Criteria {
            target,
            cover_by: cover_by.into(),
            min_stay,
        };
        if situation {
            inner.as_situation(criteria).map_err(err)?;
        } else if criteria != Criteria::default() {
            return Err(PyValueError::new_err(format!(
                "OddModule({name}): target, cover_by and min_stay need situation=True"
            )));
        }
        Ok(Module { inner })
    }

    /// Cover the module as a situation.
    #[pyo3(signature = (*, target=1.0, cover_by="hits", min_stay=None))]
    fn as_situation(&mut self, target: f64, cover_by: &str, min_stay: Option<f64>) -> PyResult<()> {
        self.inner
            .as_situation(Criteria {
                target,
                cover_by: cover_by.into(),
                min_stay,
            })
            .map_err(err)
    }

    #[getter]
    fn name(&self) -> String {
        self.inner.name.clone()
    }
    #[getter]
    fn labels(&self) -> Vec<String> {
        self.inner.labels.clone()
    }
    #[getter]
    fn active(&self) -> bool {
        self.inner.active
    }
    #[getter]
    fn text(&self) -> String {
        self.inner.text.clone()
    }
    #[getter]
    fn item(&self) -> Option<Bucketing> {
        self.inner.item.clone().map(|inner| Bucketing { inner })
    }
    #[getter]
    fn situation(&self) -> bool {
        self.inner.situation()
    }
    /// `(op, [condition, ...])` of the include and exclude sections.
    #[getter]
    fn sections(&self) -> Vec<(String, Vec<Condition>)> {
        [
            ("include", &self.inner.include),
            ("exclude", &self.inner.exclude),
        ]
        .into_iter()
        .filter_map(|(kind, s)| {
            s.as_ref().map(|s| {
                (
                    format!("{kind}_{}", if s.all { "and" } else { "or" }),
                    s.conditions
                        .iter()
                        .cloned()
                        .map(|inner| Condition { inner })
                        .collect(),
                )
            })
        })
        .collect()
    }
    fn describe_json(&self) -> String {
        self.inner.describe().to_string()
    }
    fn metadata_json(&self) -> String {
        serde_json::to_string(&self.inner.metadata).expect("JSON")
    }
}

// ---------------------------------------------------------------------------
// The ODD
// ---------------------------------------------------------------------------

/// An ODD, and what it was read from.
#[pyclass(frozen, module = "openodd._core")]
struct Odd {
    inner: model::Odd,
    transmission: Option<Transmission>,
    sources: Vec<crate::transmission::Origin>,
    version: Option<String>,
}

impl Odd {
    fn from_loaded(l: Loaded) -> Self {
        Odd {
            inner: l.odd,
            transmission: Some(l.transmission),
            sources: l.sources,
            version: l.version,
        }
    }
}

#[pymethods]
impl Odd {
    #[new]
    #[pyo3(signature = (name, attributes, modules=None, *, roots=None, text=""))]
    fn new(
        py: Python<'_>,
        name: String,
        attributes: Vec<Py<Attribute>>,
        modules: Option<Vec<Py<Module>>>,
        roots: Option<Vec<String>>,
        text: &str,
    ) -> PyResult<Self> {
        let attributes = attributes
            .iter()
            .map(|a| a.borrow(py).inner.clone())
            .collect();
        let modules = modules
            .unwrap_or_default()
            .iter()
            .map(|m| m.borrow(py).inner.clone())
            .collect();
        let inner = model::Odd::new(name, attributes, modules, roots, text).map_err(err)?;
        Ok(Odd {
            inner,
            transmission: None,
            sources: vec![],
            version: None,
        })
    }

    #[getter]
    fn name(&self) -> String {
        self.inner.name.clone()
    }
    #[getter]
    fn text(&self) -> String {
        self.inner.text.clone()
    }
    #[getter]
    fn roots(&self) -> Vec<String> {
        self.inner.roots.clone()
    }
    #[getter]
    fn attributes(&self) -> Vec<Attribute> {
        self.inner
            .attributes
            .iter()
            .cloned()
            .map(|inner| Attribute { inner })
            .collect()
    }
    #[getter]
    fn modules(&self) -> Vec<Module> {
        self.inner
            .modules
            .iter()
            .cloned()
            .map(|inner| Module { inner })
            .collect()
    }
    /// What the root modules declare as `version` in their METADATA.
    #[getter]
    fn version(&self) -> Option<String> {
        self.version.clone()
    }
    /// The revision: the digest of the files the ODD was read from (`None`
    /// for an ODD built in code).
    #[getter]
    fn revision(&self) -> Option<String> {
        self.transmission.as_ref().map(Transmission::revision)
    }
    /// A digest of what the ODD is once built, whatever it was read from.
    #[getter]
    fn structure_digest(&self) -> String {
        self.inner.structure_digest()
    }

    /// `(inside, assumed, {module: True | False | None | "inactive"})`.
    fn evaluate<'py>(
        &self,
        py: Python<'py>,
        values: &Bound<'py, PyDict>,
    ) -> PyResult<(bool, bool, Bound<'py, PyDict>)> {
        let verdict = self.inner.evaluate(&to_values(values)?);
        let modules = PyDict::new(py);
        for (name, t) in verdict.modules {
            modules.set_item(name, truth(py, t)?)?;
        }
        Ok((verdict.inside, verdict.assumed, modules))
    }

    /// `values` with every derived attribute worked out (a new dict).
    fn complete<'py>(
        &self,
        py: Python<'py>,
        values: &Bound<'py, PyDict>,
    ) -> PyResult<Bound<'py, PyDict>> {
        let mut v = to_values(values)?;
        self.inner.complete(&mut v);
        let out = values.copy()?;
        for a in &self.inner.attributes {
            if a.derived() {
                out.set_item(&a.name, from_value(py, &v[&a.name])?)?;
            }
        }
        Ok(out)
    }

    fn outside_buckets(&self, attribute: &str) -> Vec<String> {
        self.inner.outside_buckets(attribute)
    }

    fn unmeasured(&self) -> Vec<String> {
        self.inner.unmeasured()
    }

    /// The ODD's definition as JSON.
    fn definition_json(&self) -> String {
        self.inner.definition().to_string()
    }

    /// The transmission's manifest as JSON (`None` for an ODD built in code).
    fn manifest_json(&self) -> Option<String> {
        self.transmission
            .as_ref()
            .map(|t| serde_json::to_string(&t.manifest()).expect("JSON"))
    }

    /// The git sources read, as JSON.
    fn sources_json(&self) -> String {
        serde_json::to_string(&self.sources).expect("JSON")
    }

    /// `[(name, sha256, text, origin JSON or None)]` of the files read.
    fn files(&self) -> Vec<(String, String, String, Option<String>)> {
        self.transmission
            .iter()
            .flat_map(|t| t.files.iter())
            .map(|f| {
                (
                    f.name.clone(),
                    f.sha256(),
                    f.text(),
                    f.origin
                        .as_ref()
                        .map(|o| serde_json::to_string(o).expect("JSON")),
                )
            })
            .collect()
    }

    /// The transmission as a deterministic `.tar.gz`.
    fn archive<'py>(&self, py: Python<'py>) -> PyResult<Option<Bound<'py, PyBytes>>> {
        match &self.transmission {
            None => Ok(None),
            Some(t) => Ok(Some(PyBytes::new(py, &t.to_archive().map_err(err)?))),
        }
    }

    fn __repr__(&self) -> String {
        format!("OddDefinition({:?})", self.inner.name)
    }
}

// ---------------------------------------------------------------------------
// Loading
// ---------------------------------------------------------------------------

/// A Python backend as a resolver: an object with `reader_keys()`,
/// `probe_unit(spec)` and `reader_unit(key, value)` (raising ValueError).
struct PyResolver(PyObject);

impl Resolver for PyResolver {
    fn reader_keys(&self) -> Vec<String> {
        Python::with_gil(|py| {
            self.0
                .call_method0(py, "reader_keys")
                .and_then(|r| r.extract(py))
                .unwrap_or_default()
        })
    }
    fn probe_unit(&self, spec: &str) -> Result<String, String> {
        Python::with_gil(|py| match self.0.call_method1(py, "probe_unit", (spec,)) {
            Ok(r) => r.extract::<String>(py).map_err(|e| e.to_string()),
            Err(e) => Err(e.value(py).to_string()),
        })
    }
    fn reader_unit(&self, key: &str, value: &str) -> Result<String, String> {
        Python::with_gil(
            |py| match self.0.call_method1(py, "reader_unit", (key, value)) {
                Ok(r) => r.extract::<String>(py).map_err(|e| e.to_string()),
                Err(e) => Err(e.value(py).to_string()),
            },
        )
    }
}

fn yaml_node(text: Option<&str>) -> PyResult<Option<Node>> {
    match text {
        None => Ok(None),
        Some(t) => Ok(parse_all(t).map_err(err)?.into_iter().next()),
    }
}

#[pyfunction]
fn load_binding(path: PathBuf, resolver: PyObject) -> PyResult<Odd> {
    loader::load_binding(&path, &PyResolver(resolver))
        .map(Odd::from_loaded)
        .map_err(err)
}

#[pyfunction]
fn load_file(path: PathBuf, resolver: PyObject) -> PyResult<Odd> {
    loader::load_file(&path, &PyResolver(resolver))
        .map(Odd::from_loaded)
        .map_err(err)
}

/// `sources`: paths (`str` or `os.PathLike`), YAML text, or `(url, rev, path)`
/// for a file in git. `bindings` and `situations` as YAML (or JSON) text.
#[pyfunction]
#[pyo3(signature = (sources, resolver, *, bindings=None, name=None, text="", situations=None))]
fn load_openodd(
    sources: &Bound<'_, PyList>,
    resolver: PyObject,
    bindings: Option<&str>,
    name: Option<String>,
    text: &str,
    situations: Option<&str>,
) -> PyResult<Odd> {
    let mut srcs = Vec::new();
    for s in sources.iter() {
        if let Ok((url, rev, path)) = s.extract::<(String, String, String)>() {
            srcs.push(Source::Git(GitSource::new(url, rev, path).map_err(err)?));
        } else if let Ok(t) = s.extract::<String>() {
            srcs.push(Source::Text(t));
        } else {
            srcs.push(Source::Path(s.extract::<PathBuf>()?));
        }
    }
    let options = Options {
        bindings: yaml_node(bindings)?,
        name,
        text: text.into(),
        situations: yaml_node(situations)?,
    };
    loader::load_openodd(&srcs, &options, &PyResolver(resolver))
        .map(Odd::from_loaded)
        .map_err(err)
}

/// Read an ODD back from its transmission archive.
#[pyfunction]
fn load_archive(archive: &[u8], resolver: PyObject) -> PyResult<Odd> {
    let t = Transmission::from_archive(archive).map_err(err)?;
    loader::load_transmission(&t, &PyResolver(resolver))
        .map(Odd::from_loaded)
        .map_err(err)
}

/// The manifest (JSON) of a transmission archive, without reading the ODD.
#[pyfunction]
fn archive_manifest(archive: &[u8]) -> PyResult<String> {
    let t = Transmission::from_archive(archive).map_err(err)?;
    Ok(serde_json::to_string(&t.manifest()).expect("JSON"))
}

#[pyfunction]
#[pyo3(signature = (value, from_unit, to_unit))]
fn convert(value: f64, from_unit: &str, to_unit: &str) -> PyResult<f64> {
    crate::units::convert(value, from_unit, to_unit).map_err(err)
}

#[pyfunction]
fn normalize_unit(unit: &str) -> String {
    crate::units::normalize_unit(unit)
}

#[pyfunction]
fn value_label(value: &Bound<'_, PyAny>) -> PyResult<String> {
    Ok(to_value(value)?.label())
}

// ---------------------------------------------------------------------------
// Coverage reports
// ---------------------------------------------------------------------------

/// An item or a cross merged over runs.
#[pyclass(frozen, module = "openodd._core")]
#[derive(Clone)]
struct MergedEntry {
    inner: crate::report::MergedEntry,
}

#[pymethods]
impl MergedEntry {
    #[getter]
    fn name(&self) -> String {
        self.inner.name.clone()
    }
    #[getter]
    fn group(&self) -> String {
        self.inner.group.clone()
    }
    #[getter]
    fn kind(&self) -> String {
        self.inner.kind.clone()
    }
    #[getter]
    fn event(&self) -> String {
        self.inner.event.clone()
    }
    #[getter]
    fn text(&self) -> String {
        self.inner.text.clone()
    }
    #[getter]
    fn target(&self) -> f64 {
        self.inner.target
    }
    #[getter]
    fn buckets(&self) -> Vec<String> {
        self.inner.buckets.clone()
    }
    #[getter]
    fn unit(&self) -> String {
        self.inner.unit.clone()
    }
    #[getter]
    fn items(&self) -> Vec<String> {
        self.inner.items.clone()
    }
    #[getter]
    fn hits(&self) -> HashMap<String, i64> {
        self.inner.hits.clone().into_iter().collect()
    }
    #[getter]
    fn runs(&self) -> HashMap<String, i64> {
        self.inner.runs.clone().into_iter().collect()
    }
    #[getter]
    fn hit_by(&self) -> HashMap<String, Vec<usize>> {
        self.inner.hit_by.clone().into_iter().collect()
    }
    #[getter]
    fn out_of_range(&self) -> HashMap<String, i64> {
        self.inner.out_of_range.clone().into_iter().collect()
    }
    #[getter]
    fn samples(&self) -> i64 {
        self.inner.samples
    }
    #[getter]
    fn ignored(&self) -> i64 {
        self.inner.ignored
    }
    #[getter]
    fn outside(&self) -> Vec<String> {
        self.inner.outside.clone()
    }
    #[getter]
    fn cover_by(&self) -> String {
        self.inner.cover_by.clone()
    }
    #[getter]
    fn min_stay(&self) -> Option<f64> {
        self.inner.min_stay
    }
    #[getter]
    fn targets(&self) -> Vec<String> {
        self.inner.targets()
    }
    #[getter]
    fn covered(&self) -> Vec<String> {
        self.inner.covered()
    }
    #[getter]
    fn holes(&self) -> Vec<String> {
        self.inner.holes()
    }
    #[getter]
    fn grade(&self) -> f64 {
        self.inner.grade()
    }
    fn amount(&self, bucket: &str) -> f64 {
        self.inner.amount(bucket)
    }
    fn is_covered(&self, bucket: &str) -> bool {
        self.inner.is_covered(bucket)
    }
    fn exposure_of(&self, bucket: &str) -> HashMap<String, Option<f64>> {
        self.inner.exposure_of(bucket).into_iter().collect()
    }
    fn to_json(&self) -> String {
        self.inner.to_json().to_string()
    }
    fn __repr__(&self) -> String {
        format!("MergedEntry({:?})", self.inner.name)
    }
}

/// Coverage merged over a number of runs.
#[pyclass(frozen, module = "openodd._core")]
#[derive(Clone)]
struct CoverageReport {
    inner: crate::report::CoverageReport,
}

#[pymethods]
impl CoverageReport {
    #[getter]
    fn runs(&self) -> i64 {
        self.inner.runs
    }
    #[getter]
    fn scenarios(&self) -> HashMap<String, i64> {
        self.inner.scenarios.clone().into_iter().collect()
    }
    #[getter]
    fn entries(&self) -> Vec<MergedEntry> {
        self.inner
            .entries
            .iter()
            .cloned()
            .map(|inner| MergedEntry { inner })
            .collect()
    }
    fn groups(&self) -> Vec<String> {
        self.inner.groups()
    }
    fn group_grade(&self, group: &str) -> f64 {
        self.inner.group_grade(group)
    }
    /// ODD name -> its exposure, as JSON.
    fn odds_json(&self) -> String {
        serde_json::Value::Object(
            self.inner
                .odds
                .iter()
                .map(|(k, v)| (k.clone(), v.to_json()))
                .collect(),
        )
        .to_string()
    }
    fn to_json(&self) -> String {
        self.inner.to_json().to_string()
    }
    #[pyo3(signature = (max_holes=20))]
    fn to_markdown(&self, max_holes: usize) -> String {
        self.inner.to_markdown(max_holes)
    }
}

/// Merge coverage documents (a JSON list of the files' contents).
#[pyfunction]
fn merge_coverage(documents_json: &str) -> PyResult<CoverageReport> {
    let docs: Vec<serde_json::Value> =
        serde_json::from_str(documents_json).map_err(|e| PyValueError::new_err(e.to_string()))?;
    Ok(CoverageReport {
        inner: crate::report::merge_coverage(&docs).map_err(err)?,
    })
}

// ---------------------------------------------------------------------------
// Sampling
// ---------------------------------------------------------------------------

/// How a run sets an ODD attribute.
#[pyclass(frozen, module = "openodd._core")]
#[derive(Clone)]
struct Knob {
    inner: crate::sampler::Knob,
}

#[pymethods]
impl Knob {
    /// `values`: bucket label -> a value, or `(low, high)` to draw it from.
    #[new]
    #[pyo3(signature = (key, values=None, *, scale=1.0, offset=0.0, integer=false, range=None, limits=None))]
    fn new(
        key: String,
        values: Option<&Bound<'_, PyDict>>,
        scale: f64,
        offset: f64,
        integer: bool,
        range: Option<(f64, f64)>,
        limits: Option<(f64, f64)>,
    ) -> PyResult<Self> {
        let mut inner = crate::sampler::Knob::new(key);
        for (label, v) in values.into_iter().flat_map(|d| d.iter()) {
            let value = match v.extract::<(f64, f64)>() {
                Ok((low, high)) if !v.is_instance_of::<pyo3::types::PyString>() => {
                    crate::sampler::KnobValue::Range(low, high)
                }
                _ => crate::sampler::KnobValue::Value(to_value(&v)?),
            };
            inner.values.insert(label.str()?.to_string(), value);
        }
        inner.scale = scale;
        inner.offset = offset;
        inner.integer = integer;
        inner.range = range;
        inner.limits = limits;
        inner.check().map_err(err)?;
        Ok(Knob { inner })
    }

    /// `range` within what `limits` lets be written, in the attribute's unit.
    #[getter]
    fn reach(&self) -> Option<(f64, f64)> {
        self.inner.reach()
    }
}

/// One concrete case the sampler drew.
#[pyclass(frozen, module = "openodd._core")]
#[derive(Clone)]
struct Sample {
    inner: crate::sampler::Sample,
}

#[pymethods]
impl Sample {
    #[new]
    #[pyo3(signature = (index, overrides, buckets, values, situation=None))]
    fn new(
        index: usize,
        overrides: Vec<String>,
        buckets: Vec<(String, String)>,
        values: &Bound<'_, PyDict>,
        situation: Option<String>,
    ) -> PyResult<Self> {
        let mut vals = indexmap::IndexMap::new();
        for (k, v) in values.iter() {
            vals.insert(k.str()?.to_string(), to_value(&v)?);
        }
        Ok(Sample {
            inner: crate::sampler::Sample {
                index,
                overrides,
                buckets: buckets.into_iter().collect(),
                values: vals,
                situation,
            },
        })
    }
    #[getter]
    fn index(&self) -> usize {
        self.inner.index
    }
    #[getter]
    fn overrides(&self) -> Vec<String> {
        self.inner.overrides.clone()
    }
    #[getter]
    fn buckets(&self) -> Vec<(String, String)> {
        self.inner.buckets.clone().into_iter().collect()
    }
    #[getter]
    fn values(&self, py: Python<'_>) -> PyResult<Vec<(String, PyObject)>> {
        self.inner
            .values
            .iter()
            .map(|(k, v)| Ok((k.clone(), from_value(py, v)?)))
            .collect()
    }
    #[getter]
    fn situation(&self) -> Option<String> {
        self.inner.situation.clone()
    }
}

/// Draws concrete cases inside an ODD.
#[pyclass(module = "openodd._core")]
struct Sampler {
    inner: crate::sampler::Sampler,
}

#[pymethods]
impl Sampler {
    #[new]
    #[pyo3(signature = (odd, knobs, *, seed="0", strategy="uniform", coverage=None, max_tries=200, drawn=None))]
    fn new(
        py: Python<'_>,
        odd: &Odd,
        knobs: Vec<(String, Py<Knob>)>,
        seed: &str,
        strategy: &str,
        coverage: Option<Py<CoverageReport>>,
        max_tries: usize,
        drawn: Option<Vec<Py<Sample>>>,
    ) -> PyResult<Self> {
        let knobs: indexmap::IndexMap<String, crate::sampler::Knob> = knobs
            .into_iter()
            .map(|(k, v)| (k, v.borrow(py).inner.clone()))
            .collect();
        let drawn: Vec<crate::sampler::Sample> = drawn
            .unwrap_or_default()
            .iter()
            .map(|s| s.borrow(py).inner.clone())
            .collect();
        let coverage = coverage.map(|c| c.borrow(py).inner.clone());
        let strategy = crate::sampler::Strategy::parse(strategy).map_err(err)?;
        let inner = crate::sampler::Sampler::new(
            &odd.inner,
            &knobs,
            seed,
            strategy,
            coverage.as_ref(),
            max_tries,
            &drawn,
        )
        .map_err(err)?;
        Ok(Sampler { inner })
    }
    #[getter]
    fn attributes(&self) -> Vec<String> {
        self.inner.attributes()
    }
    #[getter]
    fn situation_holes(&self) -> Vec<String> {
        self.inner.situation_holes()
    }
    /// Warnings since the last call (situations given up on).
    fn take_warnings(&mut self) -> Vec<String> {
        std::mem::take(&mut self.inner.warnings)
    }
    #[pyo3(signature = (index=0, situation=None))]
    fn draw(&mut self, index: usize, situation: Option<&str>) -> PyResult<Sample> {
        self.inner
            .draw(index, situation)
            .map(|inner| Sample { inner })
            .map_err(err)
    }
    fn sample(&mut self, count: usize) -> PyResult<Vec<Sample>> {
        Ok(self
            .inner
            .sample(count)
            .map_err(err)?
            .into_iter()
            .map(|inner| Sample { inner })
            .collect())
    }
}

#[pymodule]
fn _core(m: &Bound<'_, PyModule>) -> PyResult<()> {
    let py = m.py();
    m.add("OpenOddError", py.get_type::<OpenOddError>())?;
    m.add("UnitError", py.get_type::<UnitError>())?;
    m.add("GitSourceError", py.get_type::<GitSourceError>())?;
    m.add("UNDECIDED", Py::new(py, Undecided)?)?;
    m.add_class::<Undecided>()?;
    m.add_class::<Bucketing>()?;
    m.add_class::<Condition>()?;
    m.add_class::<Attribute>()?;
    m.add_class::<Module>()?;
    m.add_class::<Odd>()?;
    m.add_class::<MergedEntry>()?;
    m.add_class::<CoverageReport>()?;
    m.add_class::<Knob>()?;
    m.add_class::<Sample>()?;
    m.add_class::<Sampler>()?;
    for f in [
        wrap_pyfunction!(is_in, m)?,
        wrap_pyfunction!(equals, m)?,
        wrap_pyfunction!(interval, m)?,
        wrap_pyfunction!(is_unknown, m)?,
        wrap_pyfunction!(all_of, m)?,
        wrap_pyfunction!(any_of, m)?,
        wrap_pyfunction!(module_holds, m)?,
        wrap_pyfunction!(load_binding, m)?,
        wrap_pyfunction!(load_file, m)?,
        wrap_pyfunction!(load_openodd, m)?,
        wrap_pyfunction!(load_archive, m)?,
        wrap_pyfunction!(archive_manifest, m)?,
        wrap_pyfunction!(convert, m)?,
        wrap_pyfunction!(normalize_unit, m)?,
        wrap_pyfunction!(value_label, m)?,
        wrap_pyfunction!(merge_coverage, m)?,
    ] {
        m.add_function(f)?;
    }
    Ok(())
}
