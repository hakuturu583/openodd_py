//! Cells: how long a run spent in each combination of the ODD's buckets.
//!
//! Every step, each attribute with buckets (a cover item) puts its value in a
//! bucket; the tuple of those labels is the step's *cell*. A [`CellCounter`]
//! adds up the steps, seconds and metres per cell, so a consumer can slice the
//! run by any attributes afterwards without the per-step values -- and buckets
//! them exactly as the cover items do, with no second copy of the rules.
//!
//! A missing value is [`MISSING`]; a numeric value below or above every bucket
//! is [`BELOW_RANGE`] / [`ABOVE_RANGE`]; a categorical value that is none of
//! the values keeps its own label (as [`CoverItem::bucket_of`] does).

use indexmap::IndexMap;
use serde_json::{json, Value as Json};

use crate::error::{Error, Result};
use crate::items::CoverItem;
use crate::model::Odd;
use crate::value::Value;

/// The label of a step whose value was missing (or could not be bucketed).
pub const MISSING: &str = "<missing>";

/// Steps, seconds and metres per cell of an ODD's attributes.
#[derive(Debug, Clone)]
pub struct CellCounter {
    /// The attributes with buckets, in the ODD's order: the cell's labels.
    pub attributes: Vec<String>,
    /// For each of the ODD's attributes (all of them, in order), its item.
    items: Vec<Option<CoverItem>>,
    cells: IndexMap<Vec<String>, (u64, f64, f64)>,
}

fn round3(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

impl CellCounter {
    pub fn new(odd: &Odd) -> Self {
        let items: Vec<Option<CoverItem>> = odd.attributes.iter().map(|a| a.item.clone()).collect();
        let attributes = odd
            .attributes
            .iter()
            .filter(|a| a.item.is_some())
            .map(|a| a.name.clone())
            .collect();
        CellCounter {
            attributes,
            items,
            cells: IndexMap::new(),
        }
    }

    /// The cell of one step: `values` has one value per attribute of the ODD
    /// (every attribute, in the ODD's order, as samples are recorded).
    pub fn cell_of(&self, values: &[Value]) -> Result<Vec<String>> {
        if values.len() != self.items.len() {
            return Err(Error::value(format!(
                "cells: {} values for {} attributes",
                values.len(),
                self.items.len()
            )));
        }
        Ok(self
            .items
            .iter()
            .zip(values)
            .filter_map(|(item, value)| item.as_ref().map(|item| (item, value)))
            .map(|(item, value)| match value {
                Value::Missing | Value::Open => MISSING.to_string(),
                v => item.bucket_of(v).unwrap_or_else(|_| MISSING.to_string()),
            })
            .collect())
    }

    /// Count one step that took `seconds` and covered `meters`.
    pub fn add(&mut self, values: &[Value], seconds: f64, meters: f64) -> Result<()> {
        self.count(values, seconds, meters).map(|_| ())
    }

    /// Count one step, as [`CellCounter::add`]; the cell it was counted in.
    pub fn count(&mut self, values: &[Value], seconds: f64, meters: f64) -> Result<Vec<String>> {
        let cell = self.cell_of(values)?;
        let entry = self.cells.entry(cell.clone()).or_insert((0, 0.0, 0.0));
        entry.0 += 1;
        entry.1 += seconds.max(0.0);
        entry.2 += meters.max(0.0);
        Ok(cell)
    }

    /// Steps counted so far.
    pub fn ticks(&self) -> u64 {
        self.cells.values().map(|c| c.0).sum()
    }

    /// The cells: labels, steps, seconds, metres; most steps first, then by labels.
    pub fn rows(&self) -> Vec<(Vec<String>, u64, f64, f64)> {
        let mut rows: Vec<_> = self
            .cells
            .iter()
            .map(|(k, (t, s, m))| (k.clone(), *t, round3(*s), round3(*m)))
            .collect();
        rows.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        rows
    }

    /// `{"attributes": [...], "rows": [[[label, ...], ticks, seconds, meters], ...]}`.
    pub fn to_json(&self) -> Json {
        json!({
            "attributes": self.attributes,
            "rows": self.rows().into_iter()
                .map(|(labels, t, s, m)| json!([labels, t, s, m]))
                .collect::<Vec<_>>(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::items::{Buckets, Criteria};
    use crate::model::{Attribute, Probe};

    fn odd() -> Odd {
        let mut speed =
            Attribute::new("speed", Probe::Missing, "km/h", "", Criteria::default()).unwrap();
        speed
            .set_buckets(Buckets::Edges(vec![0.0, 30.0, 60.0]))
            .unwrap();
        let mut rain = Attribute::new("rain", Probe::Missing, "", "", Criteria::default()).unwrap();
        rain.set_buckets(Buckets::Values(vec![
            Value::Str("none".into()),
            Value::Str("heavy".into()),
        ]))
        .unwrap();
        let free = Attribute::new("free", Probe::Missing, "", "", Criteria::default()).unwrap();
        Odd::new("t", vec![speed, free, rain], vec![], None, "").unwrap()
    }

    #[test]
    fn buckets_like_the_items() {
        let c = CellCounter::new(&odd());
        assert_eq!(c.attributes, vec!["speed", "rain"]);
        let cell = |v: Vec<Value>| c.cell_of(&v).unwrap();
        assert_eq!(
            cell(vec![
                Value::Float(30.0),
                Value::Int(3),
                Value::Str("none".into())
            ]),
            vec!["[30, 60]", "none"]
        );
        assert_eq!(
            cell(vec![
                Value::Float(-1.0),
                Value::Missing,
                Value::Str("fog".into())
            ]),
            vec!["<below>", "fog"]
        );
        assert_eq!(
            cell(vec![Value::Float(61.0), Value::Missing, Value::Missing]),
            vec!["<above>", MISSING]
        );
        assert_eq!(
            cell(vec![Value::Str("x".into()), Value::Missing, Value::Open]),
            vec![MISSING, MISSING]
        );
        assert!(c.cell_of(&[Value::Missing]).is_err());
    }

    #[test]
    fn counts_and_orders() {
        let mut c = CellCounter::new(&odd());
        let a = vec![
            Value::Float(10.0),
            Value::Missing,
            Value::Str("none".into()),
        ];
        let b = vec![
            Value::Float(40.0),
            Value::Missing,
            Value::Str("heavy".into()),
        ];
        let z = vec![
            Value::Float(5.0),
            Value::Missing,
            Value::Str("heavy".into()),
        ];
        c.add(&a, 0.05, 0.1).unwrap();
        c.add(&b, 0.05, 0.1).unwrap();
        c.add(&b, 0.0333333, 0.2).unwrap();
        c.add(&z, 0.05, -1.0).unwrap();
        assert_eq!(c.ticks(), 4);
        let rows = c.rows();
        assert_eq!(
            rows[0],
            (
                vec!["[30, 60]".to_string(), "heavy".to_string()],
                2,
                0.083,
                0.3
            )
        );
        // ties by labels: "[0, 30)" < ... ("heavy" < "none")
        assert_eq!(rows[1].0, vec!["[0, 30)", "heavy"]);
        assert_eq!(rows[1].3, 0.0); // negative displacement is not distance
        assert_eq!(rows[2].0, vec!["[0, 30)", "none"]);
        assert_eq!(
            c.to_json()["rows"][0],
            json!([["[30, 60]", "heavy"], 2, 0.083, 0.3])
        );
    }
}
