//! An ODD coverage matrix for openodd's coverage files, as a UI component
//! other tools embed.
//!
//! * [`data::Dataset`] reads coverage documents (`openodd.coverage/1`): each
//!   run's time per ODD cell, its outcome and where it failed.
//! * [`matrix`] lays any two ODD attributes out as a matrix inside a
//!   drill-down path, and says of each cell whether its runs pass, fail or
//!   both ([`matrix::CellState`]), with [`stats`]' thresholds.
//! * [`html`] renders it: a fragment for a host page (host-driven drill-down),
//!   or a self-contained explorer with the levels below rendered ahead of
//!   time; plus the scoped CSS and the small script they need.
//! * [`cases`] reads the drawn cases of a sweep (each run's `case`) and
//!   groups the runs of each; [`case_plot`] renders them as a scatter over
//!   any two attributes, with a table of the cases run more than once, on
//!   the same styles and script; [`prediction`] is the data of its
//!   background, what a model of the runs (openodd's) predicts.
//!
//! The crate has no I/O and no framework: the host gives it parsed JSON and
//! gets strings back (or [`matrix::Matrix::to_json`] to draw it its own way).
//!
//! ```
//! use openodd_coverage_view::{data::Dataset, html, matrix::Query};
//! let docs: Vec<serde_json::Value> = vec![]; // the coverage files' contents
//! let data = Dataset::from_documents(&docs);
//! let m = data.matrix(&Query::default());
//! let fragment = html::fragment(&data, &m, &html::Options::default());
//! assert!(fragment.contains("oo-cov"));
//! ```

pub mod case_plot;
pub mod cases;
pub mod data;
pub mod drift;
pub mod html;
pub mod matrix;
pub mod prediction;
pub mod stats;

pub use cases::{CaseQuery, CaseSet, Plot, Verdict};
pub use data::Dataset;
pub use matrix::{Cell, CellState, Matrix, Query};
