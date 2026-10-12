//! Cover items: the buckets a value is put in, and what covering one takes.
//!
//! The model follows `cover()` in ASAM OpenSCENARIO DSL (section 7.5): `range`
//! with `every` makes equal buckets, `buckets` gives the edges, `values` makes
//! one bucket per value; `target` is how much a bucket needs to be covered.
//! Beyond the DSL, an item sampled every step can count its target in seconds,
//! metres or entries (`cover_by`), and leave out stays under `min_stay`.
//!
//! What samples an item -- a simulator's tick loop, a log reader -- belongs to
//! the backend; the item only says which bucket a value falls in.

use serde_json::{json, Value as Json};

use crate::error::{Error, Result};
use crate::value::{format_g, Value};

/// Pseudo-bucket a numeric sample below the first bucket is counted in.
pub const BELOW_RANGE: &str = "<below>";
/// Pseudo-bucket a numeric sample above the last bucket is counted in.
pub const ABOVE_RANGE: &str = "<above>";
/// The one bucket of a situation's cover item: the ODD module held.
pub const HOLDS: &str = "holds";
/// What an item's `target` counts.
pub const COVER_MEASURES: [&str; 4] = ["hits", "seconds", "meters", "entries"];
/// The event an item sampled on every step is sampled on.
pub const TICK: &str = "tick";

/// Which report a cover item belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoverGroup {
    /// Which operating conditions the runs exercised.
    Odd,
    /// ODD modules covered as situations: combinations the runs should drive.
    Situation,
    /// A scenario's own parameters.
    Scenario,
}

impl CoverGroup {
    pub fn as_str(self) -> &'static str {
        match self {
            CoverGroup::Odd => "odd",
            CoverGroup::Situation => "situation",
            CoverGroup::Scenario => "scenario",
        }
    }
}

/// What covering a bucket takes.
#[derive(Debug, Clone, PartialEq)]
pub struct Criteria {
    pub target: f64,
    pub cover_by: String,
    pub min_stay: Option<f64>,
}

impl Default for Criteria {
    fn default() -> Self {
        Criteria {
            target: 1.0,
            cover_by: "hits".into(),
            min_stay: None,
        }
    }
}

impl Criteria {
    /// The measure a bucket's coverage is read from: `cover_by`, or with a
    /// `min_stay` the stays long enough to count (`counted.<cover_by>`).
    pub fn amount(&self) -> String {
        if self.min_stay.is_some() {
            format!("counted.{}", self.cover_by)
        } else {
            self.cover_by.clone()
        }
    }

    /// Refuse a criterion that cannot be met or measured on `event`.
    pub fn check(&self, what: &str, event: &str) -> Result<()> {
        if !COVER_MEASURES.contains(&self.cover_by.as_str()) {
            return Err(Error::value(format!(
                "{what}: cover_by must be one of ['hits', 'seconds', 'meters', 'entries']"
            )));
        }
        if matches!(self.cover_by.as_str(), "hits" | "entries")
            && (self.target < 1.0 || self.target.fract() != 0.0)
        {
            return Err(Error::value(format!(
                "{what}: target must be a whole number, at least 1"
            )));
        }
        if !(self.target > 0.0) {
            return Err(Error::value(format!("{what}: target must be positive")));
        }
        if let Some(m) = self.min_stay {
            if !(m >= 0.0) {
                return Err(Error::value(format!(
                    "{what}: min_stay must not be negative"
                )));
            }
        }
        let timed =
            matches!(self.cover_by.as_str(), "seconds" | "meters") || self.min_stay.is_some();
        if timed && event != TICK {
            return Err(Error::value(format!(
                "{what}: cover_by='{}'{} need an item sampled on SamplingEvent.TICK: a one-shot \
                 sample has no duration or distance",
                self.cover_by,
                if self.min_stay.is_some() {
                    " and min_stay"
                } else {
                    ""
                }
            )));
        }
        Ok(())
    }
}

/// How an item's buckets are given.
#[derive(Debug, Clone, PartialEq)]
pub enum Buckets {
    /// One bucket per value.
    Values(Vec<Value>),
    /// Explicit edges: `N` edges make `N - 1` buckets.
    Edges(Vec<f64>),
    /// Equal buckets of width `every` over `[low, high]`.
    Range { low: f64, high: f64, every: f64 },
}

/// One cover item.
#[derive(Debug, Clone, PartialEq)]
pub struct CoverItem {
    pub name: String,
    pub unit: String,
    pub text: String,
    pub event: String,
    pub group: CoverGroup,
    pub criteria: Criteria,
    /// The values of a categorical item, one bucket each; empty for a numeric one.
    pub values: Vec<Value>,
    /// Bucket edges of a numeric item, ascending; empty for a categorical one.
    pub edges: Vec<f64>,
    /// Bucket labels, in order.
    pub labels: Vec<String>,
    /// The ODD attribute the item buckets, when it is one's (`odd.<name>`).
    pub attribute: Option<String>,
}

fn duplicates(names: &[String]) -> Vec<String> {
    let mut seen = std::collections::BTreeMap::<&str, usize>::new();
    for n in names {
        *seen.entry(n).or_default() += 1;
    }
    seen.into_iter()
        .filter(|(_, c)| *c > 1)
        .map(|(n, _)| n.to_string())
        .collect()
}

impl CoverItem {
    pub fn new(
        name: impl Into<String>,
        buckets: Buckets,
        event: impl Into<String>,
        group: CoverGroup,
        criteria: Criteria,
    ) -> Result<Self> {
        let name = name.into();
        let event = event.into();
        if name.is_empty() {
            return Err(Error::value("cover: name must not be empty"));
        }
        criteria.check(&format!("cover({name})"), &event)?;
        let mut item = CoverItem {
            name,
            unit: String::new(),
            text: String::new(),
            event,
            group,
            criteria,
            values: Vec::new(),
            edges: Vec::new(),
            labels: Vec::new(),
            attribute: None,
        };
        let name = item.name.clone();
        match buckets {
            Buckets::Values(values) => {
                item.labels = values.iter().map(Value::label).collect();
                if item.labels.is_empty() {
                    return Err(Error::value(format!(
                        "cover({name}): values must not be empty"
                    )));
                }
                if !duplicates(&item.labels).is_empty() {
                    return Err(Error::value(format!(
                        "cover({name}): values must be distinct"
                    )));
                }
                item.values = values;
                return Ok(item);
            }
            Buckets::Range { low, high, every } => {
                if !(low < high) {
                    return Err(Error::value(format!(
                        "cover({name}): range must be (low, high)"
                    )));
                }
                if !(every > 0.0) {
                    return Err(Error::value(format!(
                        "cover({name}): every must be positive"
                    )));
                }
                let count = (((high - low) / every - 1e-9).ceil() as i64).max(1);
                item.edges = (0..count).map(|i| low + i as f64 * every).collect();
                item.edges.push(high);
            }
            Buckets::Edges(edges) => {
                if edges.len() < 2 {
                    return Err(Error::value(format!(
                        "cover({name}): buckets needs two edges or more"
                    )));
                }
                item.edges = edges;
            }
        }
        if item.edges.windows(2).any(|w| w[1] <= w[0]) {
            return Err(Error::value(format!(
                "cover({name}): bucket edges must ascend"
            )));
        }
        let last = item.edges.len() - 2;
        item.labels = item
            .edges
            .windows(2)
            .enumerate()
            .map(|(i, w)| {
                format!(
                    "[{}, {}{}",
                    format_g(w[0], 12),
                    format_g(w[1], 12),
                    if i == last { ']' } else { ')' }
                )
            })
            .collect();
        if !duplicates(&item.labels).is_empty() {
            return Err(Error::value(format!(
                "cover({name}): bucket edges too close to tell apart"
            )));
        }
        Ok(item)
    }

    /// Whether the buckets are ranges rather than values.
    pub fn numeric(&self) -> bool {
        !self.edges.is_empty()
    }

    /// The label of the bucket `value` falls in.
    ///
    /// [`BELOW_RANGE`] or [`ABOVE_RANGE`] for a numeric value outside the
    /// buckets; a categorical value that is none of the values keeps its own
    /// label. Each bucket holds its lower edge; the last its upper edge too.
    pub fn bucket_of(&self, value: &Value) -> Result<String> {
        if !self.numeric() {
            return Ok(value.label());
        }
        let x = value
            .as_float()
            .ok_or_else(|| Error::value(format!("could not convert {value} to float")))?;
        if x.is_nan() || x < self.edges[0] {
            return Ok(BELOW_RANGE.into());
        }
        if x > *self.edges.last().expect("two edges or more") {
            return Ok(ABOVE_RANGE.into());
        }
        let index = self.edges.partition_point(|e| *e <= x).saturating_sub(1);
        Ok(self.labels[index.min(self.labels.len() - 1)].clone())
    }

    /// The item's definition, as written to a coverage file.
    ///
    /// `amount` names the measure a bucket's coverage is read from (see
    /// [`Criteria::amount`]); `attribute` is there for an ODD attribute's item.
    pub fn describe(&self) -> Json {
        let mut out = json!({
            "name": self.name,
            "group": self.group.as_str(),
            "text": self.text,
            "unit": self.unit,
            "event": self.event,
            "kind": if self.numeric() { "numeric" } else { "categorical" },
            "buckets": self.labels,
            "target": number_json(self.criteria.target),
            "cover_by": self.criteria.cover_by,
            "min_stay": self.criteria.min_stay,
            "amount": self.criteria.amount(),
        });
        if let Some(attribute) = &self.attribute {
            out["attribute"] = json!(attribute);
        }
        out
    }
}

/// A number as JSON, an integral one as an integer (as Python keeps `1`).
pub fn number_json(x: f64) -> Json {
    if x.fract() == 0.0 && x.abs() < 1e15 {
        json!(x as i64)
    } else {
        json!(x)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn numeric(edges: Vec<f64>) -> CoverItem {
        CoverItem::new(
            "x",
            Buckets::Edges(edges),
            TICK,
            CoverGroup::Odd,
            Criteria::default(),
        )
        .unwrap()
    }

    #[test]
    fn numeric_buckets_hold_their_lower_edge() {
        let item = numeric(vec![0.0, 10.0, 20.0]);
        assert_eq!(item.labels, ["[0, 10)", "[10, 20]"]);
        assert_eq!(item.bucket_of(&Value::Float(10.0)).unwrap(), "[10, 20]");
        assert_eq!(item.bucket_of(&Value::Int(20)).unwrap(), "[10, 20]");
        assert_eq!(item.bucket_of(&Value::Float(-1.0)).unwrap(), BELOW_RANGE);
        assert_eq!(item.bucket_of(&Value::Float(21.0)).unwrap(), ABOVE_RANGE);
    }

    #[test]
    fn a_range_that_does_not_divide_ends_in_a_narrower_bucket() {
        let item = CoverItem::new(
            "x",
            Buckets::Range {
                low: 0.0,
                high: 25.0,
                every: 10.0,
            },
            TICK,
            CoverGroup::Odd,
            Criteria::default(),
        )
        .unwrap();
        assert_eq!(item.edges, [0.0, 10.0, 20.0, 25.0]);
    }

    #[test]
    fn timed_criteria_need_a_tick() {
        let c = Criteria {
            target: 2.0,
            cover_by: "seconds".into(),
            min_stay: None,
        };
        assert!(c.check("x", "end").is_err());
        assert!(c.check("x", TICK).is_ok());
    }
}
