//! Sample concrete cases from an ODD.
//!
//! An ODD says which situations a system is meant to handle; coverage says
//! which of them the runs so far have driven. The sampler closes the loop: it
//! draws the settings of the next runs from the ODD -- only combinations the ODD
//! admits -- and, given the coverage of earlier runs, aims them at what is still
//! uncovered.
//!
//! What can be drawn are the attributes a run *sets*: each has a [`Knob`], the
//! setting that brings it about (which the application decides: a simulator's
//! weather, a scenario's parameter). Attributes nothing sets are left open: a
//! combination is admitted when the ODD can hold for *some* value of them.
//!
//! For each case every knob's attribute gets a bucket -- one of its cover
//! item's buckets, those outside the ODD left out -- and the combination is kept
//! only if the ODD's verdict on those buckets is not "outside". Then a value is
//! drawn inside each bucket and checked against the ODD again, as a value.
//!
//! `Strategy::Coverage` draws each attribute from its least covered buckets,
//! counting the cases already drawn in this batch as if they had covered
//! theirs, so a batch works through the holes rather than piling into the
//! first. Situations still uncovered are aimed at first, one case each.
//!
//! The random stream is ChaCha8 seeded with the SHA-256 of the seed: the same
//! seed, ODD, knobs and coverage give the same cases, on any platform.

use std::collections::{HashMap, HashSet};

use indexmap::IndexMap;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use regex::Regex;
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};
use crate::flaky::{Dim, ParamSpace};
use crate::items::CoverItem;
use crate::model::{Odd, Values};
use crate::report::CoverageReport;
use crate::value::{py_float_repr, Value};

/// What a knob writes for a bucket: a value, or a range to draw it from.
#[derive(Debug, Clone, PartialEq)]
pub enum KnobValue {
    Value(Value),
    Range(f64, f64),
}

/// How a run sets an ODD attribute: the setting (a config key) that does it.
#[derive(Debug, Clone, PartialEq)]
pub struct Knob {
    /// The setting the value is written to, e.g. `environment.precipitation`.
    pub key: String,
    /// Per bucket label: the value to write, or `[low, high)` to draw it from.
    /// A numeric attribute's range replaces the bucket's interval.
    pub values: IndexMap<String, KnobValue>,
    /// A numeric value, drawn in the attribute's unit, is written as
    /// `value * scale + offset`. Must not be zero.
    pub scale: f64,
    pub offset: f64,
    /// Write an integer.
    pub integer: bool,
    /// For a numeric attribute, `[low, high]` in its unit: what the run can bring about.
    pub range: Option<(f64, f64)>,
    /// `[low, high]` the written value must lie in.
    pub limits: Option<(f64, f64)>,
}

impl Knob {
    pub fn new(key: impl Into<String>) -> Self {
        Knob {
            key: key.into(),
            values: IndexMap::new(),
            scale: 1.0,
            offset: 0.0,
            integer: false,
            range: None,
            limits: None,
        }
    }

    /// Refuse a knob that cannot write anything.
    pub fn check(&self) -> Result<()> {
        if self.key.is_empty() {
            return Err(Error::value("OddKnob: key must not be empty"));
        }
        if self.scale == 0.0 {
            return Err(Error::value(format!(
                "OddKnob({}): scale must not be zero",
                self.key
            )));
        }
        for (what, r) in [("range", self.range), ("limits", self.limits)] {
            if let Some((low, high)) = r {
                if !(low <= high) {
                    return Err(Error::value(format!(
                        "OddKnob({}): {what} must be [low, high]",
                        self.key
                    )));
                }
            }
        }
        for (label, v) in &self.values {
            if let KnobValue::Range(low, high) = v {
                if !(low <= high) {
                    return Err(Error::value(format!(
                        "OddKnob({}): range for '{label}' must be [low, high]",
                        self.key
                    )));
                }
            }
        }
        Ok(())
    }

    /// `range` within what `limits` lets be written, in the attribute's unit.
    pub fn reach(&self) -> Option<(f64, f64)> {
        let Some((l0, l1)) = self.limits else {
            return self.range;
        };
        let mut ends = [
            (l0 - self.offset) / self.scale,
            (l1 - self.offset) / self.scale,
        ];
        ends.sort_by(f64::total_cmp);
        let (mut low, mut high) = (ends[0], ends[1]);
        if let Some((r0, r1)) = self.range {
            low = low.max(r0);
            high = high.min(r1);
        }
        Some((low, high))
    }
}

/// One concrete case the sampler drew.
#[derive(Debug, Clone, PartialEq)]
pub struct Sample {
    /// 0-based position in the batch.
    pub index: usize,
    /// The settings that bring it about, `key=value` each.
    pub overrides: Vec<String>,
    /// Attribute name -> the bucket label drawn for it.
    pub buckets: IndexMap<String, String>,
    /// Attribute name -> the value drawn, in the attribute's terms.
    pub values: IndexMap<String, Value>,
    /// The situation the case was aimed at, if any.
    pub situation: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strategy {
    /// Every admitted bucket alike.
    Uniform,
    /// The least covered first.
    Coverage,
}

impl Strategy {
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "uniform" => Ok(Strategy::Uniform),
            "coverage" => Ok(Strategy::Coverage),
            other => Err(Error::value(format!(
                "OddSampler: strategy must be 'uniform' or 'coverage', not '{other}'"
            ))),
        }
    }
}

#[derive(Debug, Clone)]
struct Axis {
    attribute: String,
    item: CoverItem,
    knob: Knob,
    labels: Vec<String>,
}

/// Works out attributes the core cannot (a Python probe's `from_values`):
/// fills them into the values, leaving them open when it cannot tell.
pub type Derive = std::sync::Arc<dyn Fn(&mut Values) + Send + Sync>;

/// Draws concrete cases inside an ODD.
#[derive(Clone)]
pub struct Sampler {
    odd: Odd,
    strategy: Strategy,
    rng: ChaCha8Rng,
    max_tries: usize,
    axes: Vec<Axis>,
    /// Situation -> whether the knobs alone decide it.
    decided: HashMap<String, bool>,
    situations: Vec<String>,
    amount: HashMap<(String, String), f64>,
    situation_holes: Vec<String>,
    planned: HashMap<(String, String), i64>,
    /// Buckets the knobs can set but no admitted case has: not holes to fill.
    unreachable: HashSet<(String, String)>,
    /// What went wrong but did not stop the batch (a situation given up on).
    pub warnings: Vec<String>,
    /// Works out the attributes derived outside the core, if any.
    pub derive: Option<Derive>,
}

impl std::fmt::Debug for Sampler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sampler")
            .field("odd", &self.odd.name)
            .field("strategy", &self.strategy)
            .finish()
    }
}

fn rng_for(seed: &str) -> ChaCha8Rng {
    let digest: [u8; 32] = Sha256::digest(seed.as_bytes()).into();
    ChaCha8Rng::from_seed(digest)
}

/// A value the way a setting reads it back -- exactly (a float in full).
pub fn render(value: &Value) -> String {
    match value {
        Value::Bool(b) => b.to_string(),
        Value::Float(x) => py_float_repr(*x),
        other => other.label(),
    }
}

impl Sampler {
    /// `knobs`: attribute name -> how a run sets it, for the attributes that
    /// can be drawn. `drawn`: the cases drawn from this ODD by earlier samplers
    /// of the batch; their buckets count as planned and their situations as
    /// aimed at, and the stream is derived from the seed and how many there are.
    pub fn new(
        odd: &Odd,
        knobs: &IndexMap<String, Knob>,
        seed: &str,
        strategy: Strategy,
        coverage: Option<&CoverageReport>,
        max_tries: usize,
        drawn: &[Sample],
    ) -> Result<Self> {
        let unknown: Vec<&String> = knobs
            .keys()
            .filter(|k| odd.attribute(k).is_none())
            .collect();
        if !unknown.is_empty() {
            let mut names: Vec<String> = unknown.into_iter().cloned().collect();
            names.sort();
            return Err(Error::value(format!(
                "OddSampler: ODD {} has no attributes {}",
                odd.name,
                crate::model::py_list(&names)
            )));
        }
        let mut axes = Vec::new();
        for attribute in &odd.attributes {
            let Some(knob) = knobs.get(&attribute.name) else {
                continue;
            };
            knob.check()?;
            let Some(item) = &attribute.item else {
                return Err(Error::value(format!(
                    "OddSampler: {} has no buckets to draw from",
                    attribute.name
                )));
            };
            let outside: HashSet<String> =
                odd.outside_buckets(&attribute.name).into_iter().collect();
            let labels: Vec<String> = item
                .labels
                .iter()
                .filter(|l| !outside.contains(*l) && drawable(item, knob, l))
                .cloned()
                .collect();
            if labels.is_empty() {
                return Err(Error::value(format!(
                    "OddSampler: no bucket of {} is both inside ODD {} and given a value by its knob ({})",
                    attribute.name, odd.name, knob.key
                )));
            }
            axes.push(Axis {
                attribute: attribute.name.clone(),
                item: item.clone(),
                knob: knob.clone(),
                labels,
            });
        }
        if axes.is_empty() {
            return Err(Error::value(format!(
                "OddSampler: nothing of ODD {} can be drawn: no attribute is given a knob",
                odd.name
            )));
        }
        let sampled: HashSet<&str> = axes.iter().map(|a| a.attribute.as_str()).collect();
        let mut decided = HashMap::new();
        let situations: Vec<String> = odd
            .modules
            .iter()
            .filter(|m| m.situation() && m.active)
            .map(|m| m.name.clone())
            .collect();
        for m in odd.modules.iter().filter(|m| situations.contains(&m.name)) {
            let mut tested = Vec::new();
            let mut refs = std::collections::BTreeSet::new();
            for c in m.conditions() {
                c.attributes(&mut tested);
                c.references(&mut refs);
            }
            decided.insert(
                m.name.clone(),
                refs.is_empty() && tested.iter().all(|t| sampled.contains(t.as_str())),
            );
        }
        let seed_text = if drawn.is_empty() {
            seed.to_string()
        } else {
            format!("{seed}/{}", drawn.len())
        };
        let mut sampler = Sampler {
            odd: odd.clone(),
            strategy,
            rng: rng_for(&seed_text),
            max_tries,
            axes,
            decided,
            situations,
            amount: HashMap::new(),
            situation_holes: Vec::new(),
            planned: HashMap::new(),
            unreachable: HashSet::new(),
            warnings: Vec::new(),
            derive: None,
        };
        if strategy == Strategy::Coverage {
            sampler.read_coverage(coverage);
        }
        for s in drawn {
            for (name, label) in &s.buckets {
                *sampler
                    .planned
                    .entry((name.clone(), label.clone()))
                    .or_default() += 1;
            }
        }
        let aimed: HashSet<&String> = drawn.iter().filter_map(|s| s.situation.as_ref()).collect();
        sampler.situation_holes.retain(|s| !aimed.contains(s));
        Ok(sampler)
    }

    /// Remember how covered each bucket is, and which situations are holes.
    fn read_coverage(&mut self, coverage: Option<&CoverageReport>) {
        let variant = Regex::new(r"#\d+$").expect("a valid regex");
        let mut covered_situations = HashSet::new();
        for entry in coverage.map(|c| c.entries.as_slice()).unwrap_or_default() {
            let name = variant.replace(&entry.name, "").to_string();
            if let Some(module) = name.strip_prefix("odd.situation.") {
                if let Some(m) = self
                    .odd
                    .modules
                    .iter()
                    .find(|m| m.name == module && self.situations.contains(&m.name))
                {
                    let labels = m
                        .item
                        .as_ref()
                        .map(|i| i.labels.clone())
                        .unwrap_or_default();
                    if entry.buckets == labels && entry.holes().is_empty() {
                        covered_situations.insert(m.name.clone());
                    }
                    continue;
                }
            }
            let Some(attr) = name
                .strip_prefix("odd.")
                .and_then(|n| self.odd.attribute(n))
            else {
                continue;
            };
            let Some(item) = &attr.item else { continue };
            // Coverage measured some other way counts towards something else.
            if entry.buckets != item.labels || entry.cover_by != attr.criteria.cover_by {
                continue;
            }
            let target = if entry.target > 0.0 {
                entry.target
            } else {
                1.0
            };
            for b in &entry.buckets {
                *self
                    .amount
                    .entry((attr.name.clone(), b.clone()))
                    .or_default() += entry.amount(b) / target;
            }
        }
        self.situation_holes = self
            .situations
            .iter()
            .filter(|s| !covered_situations.contains(*s))
            .cloned()
            .collect();
    }

    /// The attributes drawn, in the ODD's order.
    pub fn attributes(&self) -> Vec<String> {
        self.axes.iter().map(|a| a.attribute.clone()).collect()
    }

    /// How many times over each (attribute, bucket) is covered, as read from
    /// the coverage given -- only entries that count what the ODD counts.
    pub fn coverage_amounts(&self) -> Vec<((String, String), f64)> {
        let mut out: Vec<((String, String), f64)> =
            self.amount.iter().map(|(k, v)| (k.clone(), *v)).collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// The space the drawn attributes span, normalized (see [`ParamSpace`]): a
    /// numeric attribute over what its knob can bring about in its drawable
    /// buckets, a categorical one over its drawable buckets.
    pub fn space(&self) -> ParamSpace {
        let dims = self
            .axes
            .iter()
            .map(|axis| {
                if !axis.item.numeric() {
                    return Dim::Categorical {
                        name: axis.attribute.clone(),
                        labels: axis.labels.clone(),
                    };
                }
                let (mut low, mut high) = (f64::INFINITY, f64::NEG_INFINITY);
                for label in &axis.labels {
                    let span = match axis.knob.values.get(label) {
                        Some(KnobValue::Range(l, h)) => Some((*l, *h)),
                        Some(KnobValue::Value(v)) => v.as_float().map(|x| (x, x)),
                        None => interval(&axis.item, &axis.knob, label).map(|(l, h, _)| (l, h)),
                    };
                    if let Some((l, h)) = span {
                        low = low.min(l);
                        high = high.max(h);
                    }
                }
                Dim::Numeric {
                    name: axis.attribute.clone(),
                    low,
                    high,
                }
            })
            .collect();
        ParamSpace::new(dims)
    }

    /// Count *sample*, drawn elsewhere from this ODD, as planned: the coverage
    /// strategy then draws its buckets as if this sampler had drawn it.
    pub fn plan(&mut self, sample: &Sample) {
        for (name, label) in &sample.buckets {
            *self
                .planned
                .entry((name.clone(), label.clone()))
                .or_default() += 1;
        }
    }

    /// How many more cases the coverage strategy needs before every drawable
    /// bucket is covered `depth` times, counting the coverage given and the
    /// cases planned. A case covers one bucket of each attribute, so this is
    /// what the attribute lacking most lacks. Always 0 with the uniform
    /// strategy, which does not aim at the holes.
    ///
    /// A bucket the knobs can set but no admitted case has (the ODD rules out
    /// every combination of it with what the other knobs can set) is not a
    /// hole: it is tried once, given up on with a warning, and left out from
    /// then on (see [`Sampler::unreachable`]).
    pub fn coverage_deficit(&mut self, depth: usize) -> usize {
        if self.strategy == Strategy::Uniform {
            return 0;
        }
        let mut deficit = 0;
        for axis in 0..self.axes.len() {
            let attribute = self.axes[axis].attribute.clone();
            let mut lacking = 0.0;
            for label in self.axes[axis].labels.clone() {
                let missing = (depth as f64 - self.load(&attribute, &label))
                    .max(0.0)
                    .ceil();
                if missing == 0.0
                    || self
                        .unreachable
                        .contains(&(attribute.clone(), label.clone()))
                {
                    continue;
                }
                if !self.reachable(axis, &label) {
                    self.warnings.push(format!(
                        "OddSampler: no case inside ODD {} has {attribute} in bucket {label} \
                         that the knobs can set; not aiming at it",
                        self.odd.name
                    ));
                    self.unreachable.insert((attribute.clone(), label));
                    continue;
                }
                lacking += missing;
            }
            deficit = deficit.max(lacking as usize);
        }
        deficit
    }

    /// The buckets given up on as holes, (attribute, bucket), sorted.
    pub fn unreachable(&self) -> Vec<(String, String)> {
        let mut out: Vec<_> = self.unreachable.iter().cloned().collect();
        out.sort();
        out
    }

    /// Whether a case can be drawn with *label* for the axis: tried on a copy,
    /// so the sampler's random stream and plan are untouched.
    fn reachable(&self, axis: usize, label: &str) -> bool {
        self.clone().draw_with(0, None, Some((axis, label))).is_ok()
    }

    /// The situations still uncovered, which the first cases are aimed at.
    pub fn situation_holes(&self) -> Vec<String> {
        self.situation_holes.clone()
    }

    fn load(&self, attribute: &str, label: &str) -> f64 {
        let key = (attribute.to_string(), label.to_string());
        self.amount.get(&key).copied().unwrap_or(0.0).min(1.0)
            + *self.planned.get(&key).unwrap_or(&0) as f64
    }

    fn pick(&mut self, axis: usize, least: bool) -> String {
        let labels = self.axes[axis].labels.clone();
        let attribute = self.axes[axis].attribute.clone();
        if self.strategy == Strategy::Uniform {
            return labels[self.rng.gen_range(0..labels.len())].clone();
        }
        let loads: Vec<f64> = labels.iter().map(|l| self.load(&attribute, l)).collect();
        if least {
            // A bucket given up on is no hole to aim at.
            let aimable: Vec<bool> = labels
                .iter()
                .map(|l| !self.unreachable.contains(&(attribute.clone(), l.clone())))
                .collect();
            let any = aimable.iter().any(|a| *a);
            let lowest = loads
                .iter()
                .zip(&aimable)
                .filter(|(_, a)| **a || !any)
                .map(|(l, _)| *l)
                .fold(f64::INFINITY, f64::min);
            let candidates: Vec<&String> = labels
                .iter()
                .zip(loads.iter().zip(&aimable))
                .filter(|(_, (l, a))| **l == lowest && (**a || !any))
                .map(|(b, _)| b)
                .collect();
            return candidates[self.rng.gen_range(0..candidates.len())].clone();
        }
        // The least loaded did not combine into an admitted case: any bucket,
        // the less loaded the likelier.
        let weights: Vec<f64> = loads.iter().map(|l| 1.0 / (1.0 + l)).collect();
        let total: f64 = weights.iter().sum();
        let mut r = self.rng.gen::<f64>() * total;
        for (label, w) in labels.iter().zip(&weights) {
            if r < *w {
                return label.clone();
            }
            r -= w;
        }
        labels.last().expect("labels").clone()
    }

    fn admits(&self, values: &mut Values, situation: Option<&str>, strict: bool) -> bool {
        self.odd.complete_open(values);
        if let Some(derive) = &self.derive {
            derive(values);
        }
        let verdict = self.odd.evaluate(values);
        if !verdict.inside {
            return false;
        }
        let Some(situation) = situation else {
            return true;
        };
        let holds = verdict.modules.get(situation).copied();
        if strict && self.decided.get(situation).copied().unwrap_or(false) {
            return holds == Some(crate::model::Truth::True);
        }
        holds != Some(crate::model::Truth::False)
    }

    /// A value in `[low, high)` (`]` if closed); `None` if there is none.
    fn uniform(&mut self, low: f64, high: f64, closed: bool, integer: bool) -> Option<Value> {
        if low == high {
            return Some(if integer {
                Value::Int(low as i64)
            } else {
                Value::Float(low)
            });
        }
        if integer {
            let first = low.ceil() as i64;
            let last = if closed {
                high.floor() as i64
            } else {
                high.ceil() as i64 - 1
            };
            return if first > last {
                None
            } else {
                Some(Value::Int(self.rng.gen_range(first..=last)))
            };
        }
        let mut value = low + (high - low) * self.rng.gen::<f64>();
        if !closed && value >= high {
            value = next_toward(high, low);
        }
        Some(Value::Float(value))
    }

    /// (value in the attribute's terms, value written), or `None` to draw again.
    fn draw_value(&mut self, axis: usize, label: &str) -> Option<(Value, Value)> {
        let (knob, item) = (self.axes[axis].knob.clone(), self.axes[axis].item.clone());
        let index = item.labels.iter().position(|l| l == label)?;
        let given = knob.values.get(label).cloned();
        if !item.numeric() {
            let value = item.values[index].clone();
            return match given {
                Some(KnobValue::Range(low, high)) => self
                    .uniform(low, high, false, knob.integer)
                    .map(|c| (value, c)),
                Some(KnobValue::Value(v)) => Some((value, v)),
                None => None,
            };
        }
        let (low, high, closed) = match given {
            Some(KnobValue::Range(l, h)) => (l, h, true),
            Some(KnobValue::Value(v)) => {
                let x = v.as_float()?;
                (x, x, true)
            }
            None => interval(&item, &knob, label)?,
        };
        let Value::Float(drawn) = self.uniform(low, high, closed, false)? else {
            return None;
        };
        if !drawn.is_finite() {
            return None;
        }
        let mut written = Value::Float(drawn * knob.scale + knob.offset);
        if knob.integer {
            let Value::Float(w) = written else {
                unreachable!()
            };
            written = Value::Int(w.round_ties_even() as i64);
        }
        if let Some((l0, l1)) = knob.limits {
            let w = written.as_float()?;
            if !(l0 <= w && w <= l1) {
                return None;
            }
        }
        // What a run reads back, which must still be in the bucket drawn.
        let back = crate::value::parse_float(&render(&written))?;
        let value = (back - knob.offset) / knob.scale;
        if !value.is_finite() || item.bucket_of(&Value::Float(value)).ok()? != label {
            return None;
        }
        Some((Value::Float(value), written))
    }

    /// *sample* with numeric attributes moved to new values, if the knobs can
    /// set them and the ODD admits the result: what a run of it would set and
    /// read back, its buckets updated. Nothing is planned.
    pub fn moved(&self, sample: &Sample, changes: &[(String, f64)]) -> Option<Sample> {
        let mut out = sample.clone();
        for (attribute, target) in changes {
            let axis = self.axes.iter().position(|a| &a.attribute == attribute)?;
            let Axis {
                item, knob, labels, ..
            } = &self.axes[axis];
            let label = item.bucket_of(&Value::Float(*target)).ok()?;
            if !item.numeric() || !labels.contains(&label) {
                return None;
            }
            match knob.values.get(&label) {
                Some(KnobValue::Range(l, h)) if !(*l <= *target && *target <= *h) => return None,
                Some(KnobValue::Value(_)) => return None,
                None => {
                    let (l, h, _) = interval(item, knob, &label)?;
                    if !(l <= *target && *target <= h) {
                        return None;
                    }
                }
                _ => {}
            }
            let mut written = Value::Float(target * knob.scale + knob.offset);
            if knob.integer {
                written = Value::Int(written.as_float()?.round_ties_even() as i64);
            }
            if let Some((l0, l1)) = knob.limits {
                let w = written.as_float()?;
                if !(l0 <= w && w <= l1) {
                    return None;
                }
            }
            let back = crate::value::parse_float(&render(&written))?;
            let value = (back - knob.offset) / knob.scale;
            if !value.is_finite() || item.bucket_of(&Value::Float(value)).ok()? != label {
                return None;
            }
            out.values.insert(attribute.clone(), Value::Float(value));
            out.buckets.insert(attribute.clone(), label);
            let prefix = format!("{}=", knob.key);
            let entry = out.overrides.iter_mut().find(|o| o.starts_with(&prefix))?;
            *entry = format!("{prefix}{}", render(&written));
        }
        let mut as_values: Values = self
            .odd
            .attributes
            .iter()
            .map(|a| (a.name.clone(), Value::Open))
            .collect();
        as_values.extend(out.values.iter().map(|(k, v)| (k.clone(), v.clone())));
        let situation = out.situation.clone();
        if !self.admits(&mut as_values, situation.as_deref(), true) {
            return None;
        }
        Some(out)
    }

    /// Draw one case; `situation`, if given, is a module the case aims at.
    pub fn draw(&mut self, index: usize, situation: Option<&str>) -> Result<Sample> {
        self.draw_with(index, situation, None)
    }

    /// [`Sampler::draw`], with one axis's bucket fixed if `fixed` is given.
    fn draw_with(
        &mut self,
        index: usize,
        situation: Option<&str>,
        fixed: Option<(usize, &str)>,
    ) -> Result<Sample> {
        for attempt in 0..self.max_tries {
            let least = attempt < self.max_tries / 2;
            let buckets: IndexMap<String, String> = (0..self.axes.len())
                .map(|i| {
                    let label = match fixed {
                        Some((axis, label)) if axis == i => label.to_string(),
                        _ => self.pick(i, least),
                    };
                    (self.axes[i].attribute.clone(), label)
                })
                .collect();
            let mut as_buckets: Values = self
                .odd
                .attributes
                .iter()
                .map(|a| (a.name.clone(), Value::Open))
                .collect();
            for axis in &self.axes {
                let i = axis
                    .item
                    .labels
                    .iter()
                    .position(|l| *l == buckets[&axis.attribute])
                    .expect("a label");
                as_buckets.insert(axis.attribute.clone(), Value::Bucket(i));
            }
            if !self.admits(&mut as_buckets, situation, false) {
                continue;
            }
            let mut values = IndexMap::new();
            let mut overrides = Vec::new();
            let mut complete = true;
            for i in 0..self.axes.len() {
                let label = buckets[&self.axes[i].attribute].clone();
                match self.draw_value(i, &label) {
                    Some((value, written)) => {
                        values.insert(self.axes[i].attribute.clone(), value);
                        overrides.push(format!("{}={}", self.axes[i].knob.key, render(&written)));
                    }
                    None => {
                        complete = false;
                        break;
                    }
                }
            }
            if !complete {
                continue;
            }
            let mut as_values: Values = self
                .odd
                .attributes
                .iter()
                .map(|a| (a.name.clone(), Value::Open))
                .collect();
            as_values.extend(values.iter().map(|(k, v)| (k.clone(), v.clone())));
            if !self.admits(&mut as_values, situation, true) {
                continue;
            }
            for (name, label) in &buckets {
                *self
                    .planned
                    .entry((name.clone(), label.clone()))
                    .or_default() += 1;
            }
            return Ok(Sample {
                index,
                overrides,
                buckets,
                values,
                situation: situation.map(String::from),
            });
        }
        let aim = situation
            .map(|s| format!(" aimed at situation {s}"))
            .unwrap_or_default();
        Err(Error::value(format!(
            "OddSampler: found no case{aim} inside ODD {} in {} tries",
            self.odd.name, self.max_tries
        )))
    }

    /// Draw `count` cases; with the coverage strategy, the first aim at the
    /// situations still uncovered, one each. A situation no case can bring
    /// about is given up on (see [`Sampler::warnings`]).
    pub fn sample(&mut self, count: usize) -> Result<Vec<Sample>> {
        let mut aims: Vec<String> = self.situation_holes.clone();
        let mut cases = Vec::with_capacity(count);
        for index in 0..count {
            let mut done = false;
            while !aims.is_empty() {
                let situation = aims.remove(0);
                match self.draw(index, Some(&situation)) {
                    Ok(s) => {
                        cases.push(s);
                        done = true;
                        break;
                    }
                    Err(_) => self.warnings.push(format!(
                        "OddSampler: no case brings about situation {situation}; not aiming at it"
                    )),
                }
            }
            if !done {
                cases.push(self.draw(index, None)?);
            }
        }
        Ok(cases)
    }
}

fn drawable(item: &CoverItem, knob: &Knob, label: &str) -> bool {
    if !item.numeric() {
        return knob.values.contains_key(label);
    }
    if knob.values.contains_key(label) {
        return true;
    }
    // An unbounded bucket has no interval to draw from.
    interval(item, knob, label).is_some_and(|(l, h, _)| l.is_finite() && h.is_finite())
}

/// (low, high, closed): the part of a numeric bucket the knob can reach.
fn interval(item: &CoverItem, knob: &Knob, label: &str) -> Option<(f64, f64, bool)> {
    let index = item.labels.iter().position(|l| l == label)?;
    let (mut low, mut high) = (item.edges[index], item.edges[index + 1]);
    let mut closed = index == item.labels.len() - 1;
    if let Some((r0, r1)) = knob.reach() {
        if r0 > r1 {
            return None;
        }
        if r1 < high {
            high = r1;
            closed = true;
        }
        low = low.max(r0);
        // A bucket the range only touches at an edge is not one the run can
        // drive in; a range that is one point is that point.
        let single = r0 == r1;
        if low > high || (low == high && !single) {
            return None;
        }
        if low == high && item.bucket_of(&Value::Float(low)).ok()? != label {
            return None;
        }
    }
    Some((low, high, closed))
}

/// The float next to `x` toward `y` (C's `nextafter`).
fn next_toward(x: f64, y: f64) -> f64 {
    if x.is_nan() || y.is_nan() {
        return f64::NAN;
    }
    if x == y {
        return y;
    }
    if x == 0.0 {
        return if y > 0.0 {
            f64::from_bits(1)
        } else {
            -f64::from_bits(1)
        };
    }
    let bits = x.to_bits();
    let up = (y > x) == (x > 0.0);
    f64::from_bits(if up { bits + 1 } else { bits - 1 })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::items::{Buckets, Criteria};
    use crate::model::{Attribute, Condition, Module, Predicate, Probe, Section};

    fn odd() -> Odd {
        let mut gap = Attribute::new(
            "gap",
            Probe::External {
                kind: "measure".into(),
                spec: "gap".into(),
            },
            "m",
            "",
            Criteria::default(),
        )
        .unwrap();
        gap.set_buckets(Buckets::Edges(vec![0.0, 10.0, 20.0, 40.0]))
            .unwrap();
        let mut rain = Attribute::new(
            "rain",
            Probe::External {
                kind: "probe".into(),
                spec: "rain".into(),
            },
            "",
            "",
            Criteria::default(),
        )
        .unwrap();
        rain.set_buckets(Buckets::Values(vec![
            Value::Str("none".into()),
            Value::Str("heavy".into()),
        ]))
        .unwrap();
        let close = Condition::Leaf {
            attribute: "gap".into(),
            predicate: Predicate::Interval {
                low: f64::NEG_INFINITY,
                high: 10.0,
                include_low: true,
                include_high: false,
            },
        };
        let heavy = Condition::Leaf {
            attribute: "rain".into(),
            predicate: Predicate::Equal(Value::Str("heavy".into())),
        };
        // Close in heavy rain is outside the ODD.
        let bound = Module::new(
            "bound",
            None,
            Some(Section {
                all: true,
                conditions: vec![close, heavy.clone()],
            }),
        )
        .unwrap();
        let mut wet = Module::new(
            "wet",
            Some(Section {
                all: true,
                conditions: vec![heavy],
            }),
            None,
        )
        .unwrap();
        wet.as_situation(Criteria::default()).unwrap();
        Odd::new("o", vec![gap, rain], vec![bound, wet], None, "").unwrap()
    }

    fn knobs() -> IndexMap<String, Knob> {
        let mut rain = Knob::new("env.rain");
        rain.values
            .insert("none".into(), KnobValue::Value(Value::Int(0)));
        rain.values
            .insert("heavy".into(), KnobValue::Range(70.0, 100.0));
        [
            ("gap".to_string(), Knob::new("scenario.gap_m")),
            ("rain".to_string(), rain),
        ]
        .into_iter()
        .collect()
    }

    #[test]
    fn every_case_is_inside_the_odd_and_in_its_bucket() {
        let o = odd();
        let mut s = Sampler::new(&o, &knobs(), "1", Strategy::Uniform, None, 200, &[]).unwrap();
        for case in s.sample(200).unwrap() {
            let gap = case.values["gap"].as_float().unwrap();
            assert_eq!(
                o.attribute("gap")
                    .unwrap()
                    .item
                    .as_ref()
                    .unwrap()
                    .bucket_of(&Value::Float(gap))
                    .unwrap(),
                case.buckets["gap"]
            );
            assert!(!(gap < 10.0 && case.buckets["rain"] == "heavy"), "{case:?}");
            assert_eq!(case.overrides.len(), 2);
        }
    }

    #[test]
    fn the_same_seed_draws_the_same_cases() {
        let o = odd();
        let a = Sampler::new(&o, &knobs(), "7", Strategy::Uniform, None, 200, &[])
            .unwrap()
            .sample(20)
            .unwrap();
        let b = Sampler::new(&o, &knobs(), "7", Strategy::Uniform, None, 200, &[])
            .unwrap()
            .sample(20)
            .unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn coverage_works_through_the_holes_and_aims_at_situations() {
        let o = odd();
        let mut s = Sampler::new(&o, &knobs(), "3", Strategy::Coverage, None, 200, &[]).unwrap();
        assert_eq!(s.situation_holes(), ["wet"]);
        let cases = s.sample(3).unwrap();
        assert_eq!(cases[0].situation.as_deref(), Some("wet"));
        assert_eq!(cases[0].buckets["rain"], "heavy");
        let gaps: HashSet<&String> = cases.iter().map(|c| &c.buckets["gap"]).collect();
        assert_eq!(gaps.len(), 3, "{cases:?}");
    }

    #[test]
    fn a_later_sampler_goes_on_from_the_earlier() {
        let o = odd();
        let first = Sampler::new(&o, &knobs(), "5", Strategy::Coverage, None, 200, &[])
            .unwrap()
            .sample(2)
            .unwrap();
        let mut next =
            Sampler::new(&o, &knobs(), "5", Strategy::Coverage, None, 200, &first).unwrap();
        assert!(next.situation_holes().is_empty());
        let more = next.sample(1).unwrap();
        let used: HashSet<&String> = first.iter().map(|c| &c.buckets["gap"]).collect();
        assert!(!used.contains(&more[0].buckets["gap"]));
    }

    #[test]
    fn values_render_as_they_read_back() {
        assert_eq!(render(&Value::Float(0.1)), "0.1");
        assert_eq!(render(&Value::Bool(true)), "true");
        assert_eq!(next_toward(1.0, 0.0), 0.9999999999999999);
    }
}
