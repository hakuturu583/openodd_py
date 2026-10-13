//! What the matrix is built from: the runs of a set of coverage documents.
//!
//! A coverage document (`openodd.coverage/1`, written by openodd's
//! `CoverageCollector`) holds, per run, the time it spent in every *cell* of
//! its ODD -- one bucket of each bucketed attribute (`odd.cells`) -- and,
//! when recorded, its outcome (`outcome`: whether it passed, the cell it
//! failed in, what it ran on). Each ODD attribute's cover item (an item with
//! `attribute`) gives that attribute's buckets in order and those outside the
//! ODD. That is all the matrix needs: no per-tick sample is read.

use indexmap::IndexMap;
use serde_json::Value as Json;

/// A label no bucket has: a missing value, and a number below or above every
/// bucket (as openodd's cells label them).
pub const MISSING: &str = "<missing>";
pub const BELOW_RANGE: &str = "<below>";
pub const ABOVE_RANGE: &str = "<above>";
/// Labels of no bucket, after the buckets, in this order.
pub const EXTRA_LABELS: [&str; 3] = [BELOW_RANGE, ABOVE_RANGE, MISSING];

/// An ODD attribute, as an axis of the matrix.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Axis {
    pub name: String,
    pub unit: String,
    pub text: String,
    /// Its buckets in order, then any other label a run's cells carry.
    pub labels: Vec<String>,
    /// Buckets outside the ODD.
    pub outside: Vec<String>,
}

impl Axis {
    pub fn is_outside(&self, label: &str) -> bool {
        self.outside.iter().any(|o| o == label)
    }
}

/// One run: its cells and its outcome.
#[derive(Debug, Clone, PartialEq)]
pub struct Run {
    /// The document's index in the list it was read from.
    pub document: usize,
    pub scenario: String,
    /// `None` when it recorded no outcome.
    pub passed: Option<bool>,
    pub environment: Option<String>,
    /// The attributes its cells' labels are of, in order.
    pub attributes: Vec<String>,
    /// Labels -> (ticks, seconds, metres); only cells it spent a step in.
    pub cells: IndexMap<Vec<String>, (u64, f64, f64)>,
    /// The cell it failed in, when it failed and recorded it.
    pub failed_cell: Option<Vec<String>>,
}

impl Run {
    /// The label of `attribute` in a cell of this run, if it has the attribute.
    pub fn index_of(&self, attribute: &str) -> Option<usize> {
        self.attributes.iter().position(|a| a == attribute)
    }
}

/// The runs of a set of coverage documents, and the ODD attributes they
/// measured.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Dataset {
    pub runs: Vec<Run>,
    /// Attribute name -> its axis, in the order first seen.
    pub axes: IndexMap<String, Axis>,
    /// Documents with no cells table (no ODD, or written before cells were).
    pub unsampled: usize,
}

fn strings(v: Option<&Json>) -> Option<Vec<String>> {
    v?.as_array()?
        .iter()
        .map(|x| x.as_str().map(String::from))
        .collect()
}

fn environment(raw: Option<&Json>) -> Option<String> {
    match raw {
        None | Some(Json::Null) => None,
        Some(Json::String(e)) => Some(e.clone()),
        Some(other) => Some(other.to_string()),
    }
}

impl Dataset {
    /// Read the documents (the contents of coverage files). A document
    /// without a cells table counts as unsampled; anything else malformed is
    /// skipped, row by row.
    pub fn from_documents(documents: &[Json]) -> Self {
        let mut data = Dataset::default();
        for (index, doc) in documents.iter().enumerate() {
            data.read_items(doc);
            let Some(table) = doc.pointer("/odd/cells").filter(|c| c.is_object()) else {
                data.unsampled += 1;
                continue;
            };
            let Some(attributes) = strings(table.get("attributes")) else {
                data.unsampled += 1;
                continue;
            };
            let mut cells: IndexMap<Vec<String>, (u64, f64, f64)> = IndexMap::new();
            for row in table
                .get("rows")
                .and_then(Json::as_array)
                .into_iter()
                .flatten()
            {
                let Some(labels) = strings(row.get(0)) else {
                    continue;
                };
                let ticks = row.get(1).and_then(Json::as_f64).unwrap_or(0.0);
                if labels.len() != attributes.len() || ticks <= 0.0 {
                    continue;
                }
                let num = |i: usize| row.get(i).and_then(Json::as_f64).unwrap_or(0.0).max(0.0);
                let slot = cells.entry(labels).or_insert((0, 0.0, 0.0));
                slot.0 += ticks as u64;
                slot.1 += num(2);
                slot.2 += num(3);
            }
            let outcome = doc.get("outcome").filter(|o| o.is_object());
            let passed = outcome
                .and_then(|o| o.get("passed"))
                .and_then(Json::as_bool);
            let failed_cell = if passed == Some(false) {
                strings(outcome.and_then(|o| o.get("failed_cell")))
                    .filter(|c| cells.contains_key(c))
            } else {
                None
            };
            for (i, name) in attributes.iter().enumerate() {
                let axis = data.axes.entry(name.clone()).or_insert_with(|| Axis {
                    name: name.clone(),
                    ..Default::default()
                });
                for labels in cells.keys() {
                    if !axis.labels.contains(&labels[i]) {
                        axis.labels.push(labels[i].clone());
                    }
                }
            }
            data.runs.push(Run {
                document: index,
                scenario: doc
                    .get("scenario")
                    .and_then(Json::as_str)
                    .unwrap_or("")
                    .to_string(),
                passed,
                environment: environment(outcome.and_then(|o| o.get("environment"))),
                attributes,
                cells,
                failed_cell,
            });
        }
        for axis in data.axes.values_mut() {
            order_labels(axis);
        }
        data
    }

    /// The bucket order, unit and outside buckets of each ODD attribute's
    /// cover item.
    fn read_items(&mut self, doc: &Json) {
        for item in doc
            .get("items")
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
        {
            let Some(name) = item.get("attribute").and_then(Json::as_str) else {
                continue;
            };
            let axis = self.axes.entry(name.to_string()).or_insert_with(|| Axis {
                name: name.to_string(),
                ..Default::default()
            });
            let text = |k: &str| item.get(k).and_then(Json::as_str).unwrap_or("").to_string();
            if axis.unit.is_empty() {
                axis.unit = text("unit");
            }
            if axis.text.is_empty() {
                axis.text = text("text");
            }
            // Buckets go first, in the item's order; seen labels are placed
            // after them by `order_labels`.
            let mut buckets = strings(item.get("buckets")).unwrap_or_default();
            buckets.retain(|b| !axis.labels.contains(b));
            axis.labels.splice(0..0, buckets);
            for o in strings(item.get("outside_odd")).unwrap_or_default() {
                if !axis.outside.contains(&o) {
                    axis.outside.push(o);
                }
            }
        }
    }

    /// The attributes, in the order first seen.
    pub fn attributes(&self) -> Vec<String> {
        self.axes.keys().cloned().collect()
    }

    /// The scenarios of the runs, sorted.
    pub fn scenarios(&self) -> Vec<String> {
        let mut out: Vec<String> = self.runs.iter().map(|r| r.scenario.clone()).collect();
        out.sort();
        out.dedup();
        out
    }

    /// The environments of the runs with an outcome, in the order first seen.
    pub fn environments(&self) -> Vec<Option<String>> {
        let mut out: Vec<Option<String>> = Vec::new();
        for r in self.runs.iter().filter(|r| r.passed.is_some()) {
            if !out.contains(&r.environment) {
                out.push(r.environment.clone());
            }
        }
        out
    }
}

/// Buckets first (in their order), then other labels sorted, then the extra
/// labels (below, above, missing) in that order.
fn order_labels(axis: &mut Axis) {
    let extras: Vec<String> = EXTRA_LABELS
        .iter()
        .filter(|e| axis.labels.iter().any(|l| l == *e))
        .map(|e| e.to_string())
        .collect();
    axis.labels.retain(|l| !EXTRA_LABELS.contains(&l.as_str()));
    axis.labels.extend(extras);
}
