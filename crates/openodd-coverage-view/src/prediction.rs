//! What a model of the runs predicts over the two plotted attributes: the
//! case plot's background.
//!
//! This crate fits nothing. A *predictor* (openodd's flaky model, or a
//! host's own) gives, for a [`Plot`], a [`Grid`]: the plotted axes cut into
//! cells, and per cell the predicted pass probability, flip probability,
//! instability confidence and the state they make
//! ([`crate::cases::Verdict`]). The plot shades its area by those states,
//! behind the marks. A grid is plain data, so a host that draws its own plot
//! can use the same one ([`Grid::to_json`]).

use std::collections::HashMap;

use serde_json::{json, Value as Json};

use crate::cases::{Case, Plot, Verdict};

/// How one plotted axis is cut.
#[derive(Debug, Clone, PartialEq)]
pub enum GridAxis {
    /// Cells between consecutive edges (values of the attribute), each
    /// predicted at its middle.
    Numeric { name: String, edges: Vec<f64> },
    /// One cell per band, in the plot's band order.
    Categorical { name: String, labels: Vec<String> },
}

impl GridAxis {
    pub fn name(&self) -> &str {
        match self {
            GridAxis::Numeric { name, .. } | GridAxis::Categorical { name, .. } => name,
        }
    }

    /// The number of cells along it.
    pub fn len(&self) -> usize {
        match self {
            GridAxis::Numeric { edges, .. } => edges.len().saturating_sub(1),
            GridAxis::Categorical { labels, .. } => labels.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn to_json(&self) -> Json {
        match self {
            GridAxis::Numeric { name, edges } => {
                json!({"name": name, "kind": "numeric", "edges": edges})
            }
            GridAxis::Categorical { name, labels } => {
                json!({"name": name, "kind": "categorical", "labels": labels})
            }
        }
    }
}

/// What is predicted in one cell.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Predicted {
    /// What the background's colour shows.
    pub pass_probability: f64,
    /// The flip probability, instability confidence and the state they
    /// make (flaky, passes or fails, by the predictor's rule): `None` from a
    /// predictor asked for the pass probability only, which is much cheaper
    /// (a grid is thousands of points).
    pub flip_probability: Option<f64>,
    pub instability_confidence: Option<f64>,
    pub state: Option<Verdict>,
}

/// The prediction over a plot's two axes.
#[derive(Debug, Clone, PartialEq)]
pub struct Grid {
    pub x: GridAxis,
    pub y: GridAxis,
    /// Rows along y, columns along x; `None` where nothing could be
    /// predicted (a label outside the model's space).
    pub cells: Vec<Vec<Option<Predicted>>>,
    /// The attributes not on the plot, and the value each is held at.
    pub held: Vec<(String, String)>,
    /// The runs the model was fitted to.
    pub runs: usize,
    /// The rule of the states: flaky where the posterior probability that a
    /// small change flips the outcome more often than `flip_threshold`
    /// exceeds `flaky_confidence`; else passes where the pass probability
    /// is at least one half, fails where it is below.
    pub flip_threshold: f64,
    pub flaky_confidence: f64,
}

impl Grid {
    pub fn to_json(&self) -> Json {
        json!({
            "x": self.x.to_json(),
            "y": self.y.to_json(),
            "cells": self.cells.iter().map(|row| row.iter().map(|c| c.map(|c| json!({
                "pass_probability": c.pass_probability,
                "flip_probability": c.flip_probability,
                "instability_confidence": c.instability_confidence,
                "state": c.state.map(|s| s.as_str()),
            }))).collect::<Vec<_>>()).collect::<Vec<_>>(),
            "held": self.held.iter().map(|(a, v)| json!({"attribute": a, "value": v})).collect::<Vec<_>>(),
            "runs": self.runs,
            "flip_threshold": self.flip_threshold,
            "flaky_confidence": self.flaky_confidence,
        })
    }
}

/// What gives a plot its background: openodd's flaky model, or a host's.
pub trait Predictor {
    /// The grid over `plot`'s axes, or why there is none (too few runs,
    /// runs of one outcome only, an axis the model does not know).
    fn grid(&self, plot: &Plot) -> Result<Grid, String>;
    /// The pass probability predicted at a case's own values, if any.
    fn pass_probability(&self, case: &Case) -> Option<f64>;
    /// [`Self::pass_probability`] of each case: a predictor that can do
    /// them at once (one solve) does.
    fn pass_probabilities(&self, cases: &[&Case]) -> Vec<Option<f64>> {
        cases.iter().map(|c| self.pass_probability(c)).collect()
    }
}

/// Grids computed ahead of time, by their axes, and the cases' pass
/// probabilities by case key: a predictor for a host that has the data.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Precomputed {
    pub grids: Vec<Grid>,
    pub cases: HashMap<String, f64>,
    /// Why there is no grid, when there is none for any pair.
    pub unavailable: Option<String>,
}

impl Predictor for Precomputed {
    fn grid(&self, plot: &Plot) -> Result<Grid, String> {
        // The same attribute, cut the same way (numeric or categorical).
        let fits = |g: &GridAxis, a: &Option<crate::cases::CaseAxis>| {
            a.as_ref().is_some_and(|a| {
                g.name() == a.name
                    && matches!(
                        (g, a.kind),
                        (GridAxis::Numeric { .. }, crate::cases::AxisKind::Numeric)
                            | (
                                GridAxis::Categorical { .. },
                                crate::cases::AxisKind::Categorical
                            )
                    )
            })
        };
        self.grids
            .iter()
            .find(|g| fits(&g.x, &plot.x) && fits(&g.y, &plot.y))
            .cloned()
            .ok_or_else(|| self.unavailable.clone().unwrap_or_default())
    }

    fn pass_probability(&self, case: &Case) -> Option<f64> {
        self.cases.get(&case.key).copied()
    }
}
