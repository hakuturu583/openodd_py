//! Two sets of runs on the same ODD, compared: a *baseline* (the software
//! as it was, say) and *another* set (as it is now), each measured on the
//! same ODD, laid over one another.
//!
//! * [`Comparison::matrix`] counts both sets on the **same** ODD matrix --
//!   the same axes, buckets and conditions, chosen from both sets together
//!   -- and says of each cell how its failure rate moved
//!   ([`MatrixComparison`]).
//! * [`Comparison::plot`] places the cases of both sets in one case plot:
//!   the runs of the same case (the same scenario and overrides) in either
//!   set are one mark, and each mark says what its baseline runs and its
//!   other runs did ([`CaseComparison`]).
//!
//! A cell or a case [`Change`]s by its failure rate: *regressed* when the
//! other set failed more of its runs there than the baseline, *improved*
//! when fewer, *same* when as many; *new* when only the other set ran there,
//! *dropped* when only the baseline did. [`Delta::significant`] says whether
//! the change is more than the runs' noise: the two 95% Wilson intervals do
//! not overlap.
//!
//! Comparing assumes one ODD: [`OddCheck`] says whether the two sets were
//! measured on the same ODD (its name and structure digest, as each coverage
//! document records them) and which attributes' buckets differ.

use indexmap::IndexMap;
use serde_json::{json, Value as Json};

use crate::cases::{Case, CaseQuery, CaseSet, Plot};
use crate::data::Dataset;
use crate::matrix::{Cell, Matrix, Query};
use crate::stats;

/// Which set a run is of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Side {
    /// The baseline: what the other set is compared against.
    Base,
    Other,
}

impl Side {
    pub fn as_str(&self) -> &'static str {
        match self {
            Side::Base => "base",
            Side::Other => "other",
        }
    }
}

/// How a cell's (or a case's) failure rate moved from the baseline to the
/// other set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Change {
    /// Both ran there; the other set failed a larger share.
    Regressed,
    /// Both ran there; the other set failed a smaller share.
    Improved,
    /// Both ran there; the same share failed.
    Same,
    /// Only the other set ran there (with an outcome).
    New,
    /// Only the baseline ran there.
    Dropped,
    /// Neither did.
    Neither,
}

impl Change {
    pub fn as_str(&self) -> &'static str {
        match self {
            Change::Regressed => "regressed",
            Change::Improved => "improved",
            Change::Same => "same",
            Change::New => "new",
            Change::Dropped => "dropped",
            Change::Neither => "none",
        }
    }

    /// Where it sorts: what most needs a look first.
    pub fn rank(&self) -> u8 {
        match self {
            Change::Regressed => 0,
            Change::Improved => 1,
            Change::Dropped => 2,
            Change::New => 3,
            Change::Same => 4,
            Change::Neither => 5,
        }
    }

    /// Every change, in [`Change::rank`] order.
    pub const ALL: [Change; 6] = [
        Change::Regressed,
        Change::Improved,
        Change::Dropped,
        Change::New,
        Change::Same,
        Change::Neither,
    ];
}

/// The change of one cell or case.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Delta {
    pub change: Change,
    /// The other set's failure rate minus the baseline's, when both ran.
    pub fail_rate: Option<f64>,
    /// Whether the two 95% Wilson intervals of the failure rate are apart:
    /// a change more than the runs' noise.
    pub significant: bool,
}

impl Delta {
    /// From each side's `(runs, failed)`.
    pub fn of(base: (u64, u64), other: (u64, u64)) -> Delta {
        let rate = |(runs, failed): (u64, u64)| failed as f64 / runs as f64;
        match (base.0 > 0, other.0 > 0) {
            (false, false) => Delta {
                change: Change::Neither,
                fail_rate: None,
                significant: false,
            },
            (false, true) => Delta {
                change: Change::New,
                fail_rate: None,
                significant: false,
            },
            (true, false) => Delta {
                change: Change::Dropped,
                fail_rate: None,
                significant: false,
            },
            (true, true) => {
                let d = rate(other) - rate(base);
                let (a, b) = (
                    stats::wilson(base.1, base.0, stats::Z95),
                    stats::wilson(other.1, other.0, stats::Z95),
                );
                let change = if d > 1e-12 {
                    Change::Regressed
                } else if d < -1e-12 {
                    Change::Improved
                } else {
                    Change::Same
                };
                Delta {
                    change,
                    fail_rate: Some(d),
                    significant: change != Change::Same && (a.1 < b.0 || b.1 < a.0),
                }
            }
        }
    }

    pub fn of_cells(base: &Cell, other: &Cell) -> Delta {
        Delta::of((base.runs, base.failed), (other.runs, other.failed))
    }

    pub fn to_json(&self) -> Json {
        json!({
            "change": self.change.as_str(),
            "fail_rate_delta": self.fail_rate,
            "significant": self.significant,
        })
    }
}

/// How one attribute's buckets differ between the sets.
#[derive(Debug, Clone, PartialEq)]
pub struct AttributeDifference {
    pub attribute: String,
    /// The side that has it, when only one does.
    pub only_in: Option<Side>,
    /// Each side's buckets, then those outside the ODD.
    pub base: (Vec<String>, Vec<String>),
    pub other: (Vec<String>, Vec<String>),
}

/// Whether the two sets were measured on the same ODD.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct OddCheck {
    /// Each side's ODDs: `(name, structure digest)`, in the order first
    /// seen (a document without an ODD has none).
    pub base: Vec<(String, String)>,
    pub other: Vec<(String, String)>,
    /// The ODD attributes whose buckets differ, or that one side lacks.
    pub attributes: Vec<AttributeDifference>,
}

impl OddCheck {
    /// One ODD on both sides: the same structure (or, when a side recorded
    /// none, the same name), and the same buckets for every attribute.
    pub fn same(&self) -> bool {
        fn digests(s: &[(String, String)]) -> Vec<&String> {
            let mut d: Vec<&String> = s.iter().map(|(_, d)| d).collect();
            d.sort();
            d.dedup();
            d
        }
        let (a, b) = (digests(&self.base), digests(&self.other));
        a.len() <= 1 && a == b && self.attributes.is_empty()
    }

    pub fn to_json(&self) -> Json {
        let odds = |s: &[(String, String)]| {
            s.iter()
                .map(|(n, d)| json!({"name": n, "structure": d}))
                .collect::<Vec<_>>()
        };
        json!({
            "same": self.same(),
            "base": odds(&self.base),
            "other": odds(&self.other),
            "attributes": self.attributes.iter().map(|a| json!({
                "attribute": a.attribute,
                "only_in": a.only_in.map(|s| s.as_str()),
                "base": {"buckets": a.base.0, "outside_odd": a.base.1},
                "other": {"buckets": a.other.0, "outside_odd": a.other.1},
            })).collect::<Vec<_>>(),
        })
    }
}

/// The comparison's words, for a host to translate: the sides' names and
/// how a cell or case changed. The rest are the components' own
/// ([`crate::html::Labels`], [`crate::case_plot::Labels`]).
#[derive(Debug, Clone, PartialEq)]
pub struct Labels {
    /// The sides' names.
    pub base: String,
    pub other: String,
    pub matrix_title: String,
    pub matrix_hint: String,
    pub cases_title: String,
    pub cases_hint: String,
    pub regressed: String,
    pub improved: String,
    pub same: String,
    pub new: String,
    pub dropped: String,
    pub none: String,
    pub significant: String,
    pub within_noise: String,
    pub fail_rate_change: String,
    pub change: String,
    pub not_run: String,
    pub cells_suffix: String,
    pub cases_suffix: String,
    pub coverage_change: String,
    pub coverage_change_sub: String,
    pub sides_key: String,
    pub different_odd: String,
    pub odd_attribute: String,
    pub odd_only_in: String,
    pub background: String,
    pub background_delta: String,
    pub background_none: String,
    pub delta_scale: String,
    pub delta_worse: String,
    pub delta_better: String,
    pub delta_caption: String,
    pub changed_title: String,
    pub no_changed: String,
}

impl Labels {
    /// Set one label by its field name; `false` if there is no such label.
    pub fn set(&mut self, name: &str, value: &str) -> bool {
        let slot = match name {
            "base" => &mut self.base,
            "other" => &mut self.other,
            "matrix_title" => &mut self.matrix_title,
            "matrix_hint" => &mut self.matrix_hint,
            "cases_title" => &mut self.cases_title,
            "cases_hint" => &mut self.cases_hint,
            "regressed" => &mut self.regressed,
            "improved" => &mut self.improved,
            "same" => &mut self.same,
            "new" => &mut self.new,
            "dropped" => &mut self.dropped,
            "none" => &mut self.none,
            "significant" => &mut self.significant,
            "within_noise" => &mut self.within_noise,
            "fail_rate_change" => &mut self.fail_rate_change,
            "change" => &mut self.change,
            "not_run" => &mut self.not_run,
            "cells_suffix" => &mut self.cells_suffix,
            "cases_suffix" => &mut self.cases_suffix,
            "coverage_change" => &mut self.coverage_change,
            "coverage_change_sub" => &mut self.coverage_change_sub,
            "sides_key" => &mut self.sides_key,
            "different_odd" => &mut self.different_odd,
            "odd_attribute" => &mut self.odd_attribute,
            "odd_only_in" => &mut self.odd_only_in,
            "background" => &mut self.background,
            "background_delta" => &mut self.background_delta,
            "background_none" => &mut self.background_none,
            "delta_scale" => &mut self.delta_scale,
            "delta_worse" => &mut self.delta_worse,
            "delta_better" => &mut self.delta_better,
            "delta_caption" => &mut self.delta_caption,
            "changed_title" => &mut self.changed_title,
            "no_changed" => &mut self.no_changed,
            _ => return false,
        };
        *slot = value.to_string();
        true
    }

    /// The word of a change.
    pub fn change_label(&self, change: Change) -> &str {
        match change {
            Change::Regressed => &self.regressed,
            Change::Improved => &self.improved,
            Change::Same => &self.same,
            Change::New => &self.new,
            Change::Dropped => &self.dropped,
            Change::Neither => &self.none,
        }
    }

    /// A side's name.
    pub fn side(&self, side: Side) -> &str {
        match side {
            Side::Base => &self.base,
            Side::Other => &self.other,
        }
    }
}

impl Default for Labels {
    fn default() -> Self {
        let s = |x: &str| x.to_string();
        Labels {
            base: s("Baseline"),
            other: s("Compared"),
            matrix_title: s("ODD coverage, compared"),
            matrix_hint: s("Each cell: {base} on the left, {other} on the right. Click a cell to look inside it"),
            cases_title: s("Drawn cases, compared"),
            cases_hint: s("Each mark: {base} on the left, {other} on the right"),
            regressed: s("Regressed"),
            improved: s("Improved"),
            same: s("Unchanged"),
            new: s("Newly covered"),
            dropped: s("No longer covered"),
            none: s("Run in neither"),
            significant: s("beyond the runs' noise"),
            within_noise: s("within the runs' noise"),
            fail_rate_change: s("Fail rate change"),
            change: s("Change"),
            not_run: s("not run"),
            cells_suffix: s("cells"),
            cases_suffix: s("cases"),
            coverage_change: s("Coverage"),
            coverage_change_sub: s("{new} new, {dropped} lost"),
            sides_key: s("left: {base} \u{b7} right: {other}"),
            different_odd: s(
                "The two sets were not measured on the same ODD; cells and cases are matched by name and may not mean the same.",
            ),
            odd_attribute: s("{attribute}: its buckets differ"),
            odd_only_in: s("{attribute}: only in {side}"),
            background: s("Background"),
            background_delta: s("Change"),
            background_none: s("None"),
            delta_scale: s("Predicted pass rate change"),
            delta_worse: s("worse"),
            delta_better: s("better"),
            delta_caption: s(
                "The background is how the pass rate the flaky model predicts moved, fitted to each set's runs \
                 ({base_runs} and {other_runs}): red where {other} is predicted to pass less often than {base}, \
                 green where more often. Where runs are few, both models are unsure and the change stays faint.",
            ),
            changed_title: s("Changed cases"),
            no_changed: s("No case changed between the two sets."),
        }
    }
}

fn odds(documents: &[Json]) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for doc in documents {
        let Some(odd) = doc.get("odd").filter(|o| o.is_object()) else {
            continue;
        };
        let text = |k: &str| odd.get(k).and_then(Json::as_str).unwrap_or("").to_string();
        let id = (text("name"), text("structure"));
        if !out.contains(&id) {
            out.push(id);
        }
    }
    out
}

/// Two sets of coverage documents, read together.
#[derive(Debug, Clone, PartialEq)]
pub struct Comparison {
    /// Documents `[0, split)` are the baseline's; the rest the other set's.
    pub split: usize,
    /// Both sets' runs, on axes from both.
    pub coverage: Dataset,
    /// Both sets' cases: a case run in both holds the runs of both.
    pub cases: CaseSet,
    pub odd: OddCheck,
    /// Each side's documents without a case, and with a case but no outcome.
    pub skipped: [(usize, usize); 2],
}

impl Comparison {
    pub fn new(base: &[Json], other: &[Json]) -> Self {
        let mut all = base.to_vec();
        all.extend_from_slice(other);
        let (a, b) = (
            Dataset::from_documents(base),
            Dataset::from_documents(other),
        );
        let mut attributes = Vec::new();
        let names = a
            .axes
            .keys()
            .chain(b.axes.keys().filter(|k| !a.axes.contains_key(*k)));
        for name in names {
            let of = |d: &Dataset| {
                d.axes
                    .get(name)
                    .map(|x| (x.buckets.clone(), x.outside.clone()))
            };
            let (x, y) = (of(&a), of(&b));
            if x == y {
                continue;
            }
            attributes.push(AttributeDifference {
                attribute: name.clone(),
                only_in: match (&x, &y) {
                    (Some(_), None) => Some(Side::Base),
                    (None, Some(_)) => Some(Side::Other),
                    _ => None,
                },
                base: x.unwrap_or_default(),
                other: y.unwrap_or_default(),
            });
        }
        let skipped = |docs: &[Json]| {
            let mut out = (0, 0);
            for d in docs {
                if !d.get("case").is_some_and(Json::is_object) {
                    out.0 += 1;
                } else if d
                    .pointer("/outcome/passed")
                    .and_then(Json::as_bool)
                    .is_none()
                {
                    out.1 += 1;
                }
            }
            out
        };
        Comparison {
            split: base.len(),
            coverage: Dataset::from_documents(&all),
            cases: CaseSet::from_documents(&all),
            odd: OddCheck {
                base: odds(base),
                other: odds(other),
                attributes,
            },
            skipped: [skipped(base), skipped(other)],
        }
    }

    /// The side of a document, by its index in both sets together.
    pub fn side_of(&self, document: usize) -> Side {
        if document < self.split {
            Side::Base
        } else {
            Side::Other
        }
    }

    /// One side's runs, on both sides' axes.
    pub fn coverage_of(&self, side: Side) -> Dataset {
        Dataset {
            runs: self
                .coverage
                .runs
                .iter()
                .filter(|r| self.side_of(r.document) == side)
                .cloned()
                .collect(),
            axes: self.coverage.axes.clone(),
            unsampled: 0,
        }
    }

    /// One side's cases (only its runs of each), on both sides' axes: what
    /// a model of that side is fitted to.
    pub fn cases_of(&self, side: Side) -> CaseSet {
        let i = match side {
            Side::Base => 0,
            Side::Other => 1,
        };
        CaseSet {
            cases: self
                .cases
                .cases
                .iter()
                .filter_map(|c| self.split_case(c, side))
                .collect(),
            axes: self.cases.axes.clone(),
            documents: match side {
                Side::Base => self.split,
                Side::Other => self.cases.documents - self.split,
            },
            without_case: self.skipped[i].0,
            without_outcome: self.skipped[i].1,
        }
    }

    /// A case with only one side's runs; `None` when it has none there.
    pub fn split_case(&self, case: &Case, side: Side) -> Option<Case> {
        let runs: Vec<_> = case
            .runs
            .iter()
            .filter(|r| self.side_of(r.document) == side)
            .cloned()
            .collect();
        (!runs.is_empty()).then(|| Case {
            runs,
            ..case.clone()
        })
    }

    /// The matrix `query` asks for, both sides counted on the same axes:
    /// those chosen from both sides' runs together.
    pub fn matrix(&self, query: &Query) -> MatrixComparison {
        let union = self.coverage.matrix(query);
        let base = self.coverage_of(Side::Base).matrix_on(&union.query);
        let other = self.coverage_of(Side::Other).matrix_on(&union.query);
        let deltas = base
            .cells
            .iter()
            .zip(&other.cells)
            .map(|(a, b)| {
                a.iter()
                    .zip(b)
                    .map(|(a, b)| Delta::of_cells(a, b))
                    .collect()
            })
            .collect();
        let total = Delta::of_cells(&base.total, &other.total);
        MatrixComparison {
            union,
            base,
            other,
            deltas,
            total,
        }
    }

    /// The case plot `query` asks for, of both sides' cases together, with
    /// each case's runs split by side.
    pub fn plot(&self, query: &CaseQuery) -> CaseComparison {
        let plot = self.cases.plot(query);
        let cases = plot
            .cases
            .iter()
            .map(|c| {
                let base = self.split_case(c, Side::Base);
                let other = self.split_case(c, Side::Other);
                let count = |c: &Option<Case>| {
                    c.as_ref()
                        .map(|c| (c.runs.len() as u64, c.fails() as u64))
                        .unwrap_or((0, 0))
                };
                let delta = Delta::of(count(&base), count(&other));
                ComparedCase { base, other, delta }
            })
            .collect();
        CaseComparison {
            plot,
            cases,
            skipped: self.skipped,
        }
    }
}

/// Two matrices of the same axes, and how each cell moved.
#[derive(Debug, Clone, PartialEq)]
pub struct MatrixComparison {
    /// Both sides together: the axes and the query as resolved, and what
    /// is free to lay out.
    pub union: Matrix,
    pub base: Matrix,
    pub other: Matrix,
    /// Rows x columns, as the matrices' cells.
    pub deltas: Vec<Vec<Delta>>,
    /// The whole of the conditions.
    pub total: Delta,
}

impl MatrixComparison {
    /// How many cells (outside the ODD aside) changed each way.
    pub fn counts(&self) -> IndexMap<&'static str, usize> {
        let mut out: IndexMap<&'static str, usize> =
            Change::ALL.iter().map(|c| (c.as_str(), 0)).collect();
        for (r, row) in self.deltas.iter().enumerate() {
            for (c, d) in row.iter().enumerate() {
                if !self.union.cells[r][c].outside {
                    *out.get_mut(d.change.as_str()).unwrap() += 1;
                }
            }
        }
        out
    }

    pub fn to_json(&self) -> Json {
        let mut out = self.union.to_json();
        let obj = out.as_object_mut().expect("a matrix is an object");
        obj.insert(
            "cells".into(),
            json!(self
                .deltas
                .iter()
                .enumerate()
                .map(|(r, row)| row
                    .iter()
                    .enumerate()
                    .map(|(c, d)| {
                        let mut j = d.to_json();
                        j["outside_odd"] = json!(self.union.cells[r][c].outside);
                        j["base"] = self.base.cells[r][c].to_json();
                        j["other"] = self.other.cells[r][c].to_json();
                        j
                    })
                    .collect::<Vec<_>>())
                .collect::<Vec<_>>()),
        );
        let mut total = self.total.to_json();
        total["base"] = self.base.total.to_json();
        total["other"] = self.other.total.to_json();
        obj.insert("total".into(), total);
        obj.insert("counts".into(), json!(self.counts()));
        obj.insert(
            "states".into(),
            json!({"base": self.base.counts(), "other": self.other.counts()}),
        );
        obj.insert(
            "runs".into(),
            json!({"base": self.base.runs, "other": self.other.runs}),
        );
        obj.insert(
            "environments".into(),
            json!({"base": self.base.environments, "other": self.other.environments}),
        );
        out
    }
}

/// A case of the comparison: its runs on each side.
#[derive(Debug, Clone, PartialEq)]
pub struct ComparedCase {
    pub base: Option<Case>,
    pub other: Option<Case>,
    pub delta: Delta,
}

/// The case plot of both sides, each case's runs split by side.
#[derive(Debug, Clone, PartialEq)]
pub struct CaseComparison {
    /// Both sides' cases together: the axes, and each case where it goes.
    pub plot: Plot,
    /// As `plot.cases`.
    pub cases: Vec<ComparedCase>,
    /// Each side's documents without a case, and without an outcome.
    pub skipped: [(usize, usize); 2],
}

impl CaseComparison {
    /// How many cases changed each way.
    pub fn counts(&self) -> IndexMap<&'static str, usize> {
        let mut out: IndexMap<&'static str, usize> =
            Change::ALL.iter().map(|c| (c.as_str(), 0)).collect();
        for c in &self.cases {
            *out.get_mut(c.delta.change.as_str()).unwrap() += 1;
        }
        out
    }

    /// Runs of the cases placed, per side.
    pub fn runs(&self, side: Side) -> usize {
        self.cases
            .iter()
            .filter_map(|c| match side {
                Side::Base => c.base.as_ref(),
                Side::Other => c.other.as_ref(),
            })
            .map(|c| c.runs.len())
            .sum()
    }

    /// The cases that changed, most in need of a look first: regressed,
    /// improved, dropped, new; the larger change first within each.
    pub fn changed(&self) -> Vec<usize> {
        let mut out: Vec<usize> = (0..self.cases.len())
            .filter(|&i| !matches!(self.cases[i].delta.change, Change::Same | Change::Neither))
            .collect();
        out.sort_by(|&a, &b| {
            let (da, db) = (&self.cases[a].delta, &self.cases[b].delta);
            da.change.rank().cmp(&db.change.rank()).then(
                db.fail_rate
                    .unwrap_or(0.0)
                    .abs()
                    .total_cmp(&da.fail_rate.unwrap_or(0.0).abs()),
            )
        });
        out
    }

    pub fn to_json(&self) -> Json {
        let mut out = self.plot.to_json();
        let side = |c: &Option<Case>| {
            c.as_ref().map(|c| {
                json!({
                    "runs": c.runs.len(), "passes": c.passes(), "fails": c.fails(),
                    "verdict": c.verdict().as_str(),
                    "documents": c.runs.iter().map(|r| r.document).collect::<Vec<_>>(),
                })
            })
        };
        if let Some(cases) = out["cases"].as_array_mut() {
            for (j, c) in cases.iter_mut().zip(&self.cases) {
                j["base"] = json!(side(&c.base));
                j["other"] = json!(side(&c.other));
                j["change"] = json!(c.delta.change.as_str());
                j["fail_rate_delta"] = json!(c.delta.fail_rate);
                j["significant"] = json!(c.delta.significant);
            }
        }
        out["changes"] = json!(self.counts());
        out["runs"] = json!({"base": self.runs(Side::Base), "other": self.runs(Side::Other)});
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deltas() {
        let d = Delta::of((10, 0), (10, 10));
        assert_eq!(d.change, Change::Regressed);
        assert!(d.significant && d.fail_rate == Some(1.0));
        let d = Delta::of((2, 0), (2, 1));
        assert_eq!(d.change, Change::Regressed);
        assert!(!d.significant);
        assert_eq!(Delta::of((4, 2), (2, 0)).change, Change::Improved);
        assert_eq!(Delta::of((4, 2), (2, 1)).change, Change::Same);
        assert_eq!(Delta::of((0, 0), (2, 1)).change, Change::New);
        assert_eq!(Delta::of((3, 0), (0, 0)).change, Change::Dropped);
        assert_eq!(Delta::of((0, 0), (0, 0)).change, Change::Neither);
    }
}
