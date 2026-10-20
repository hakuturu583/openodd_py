//! ASAM OpenODD 1.0: read, evaluate, version and sample Operational Design Domains.
//!
//! The core is application-neutral: it reads OpenODD YAML (and a *binding*
//! saying how each concept is measured), judges values against the ODD with
//! OpenODD's three-valued, missing-value semantics, works out which buckets lie
//! outside it, and versions it by the transmission it was read from. What
//! reads the values -- a simulator, a recorded log -- is the application's
//! backend; the Python package `openodd` wraps this crate and calls it.

// `!(x > 0.0)` is meant: it is true for NaN, which must be refused too.
#![allow(clippy::neg_cmp_op_on_partial_ord)]

pub mod case_prediction;
pub mod cell_flakiness;
pub mod cells;
pub mod error;
pub mod flaky;
pub mod items;
pub mod loader;
pub mod model;
pub mod report;
pub mod sampler;
pub mod sources;
pub mod transmission;
pub mod units;
pub mod value;
pub mod yaml;

#[cfg(feature = "python")]
mod python;

pub use error::{Error, Result};
pub use items::{Buckets, CoverGroup, CoverItem, Criteria};
pub use loader::{
    load_binding, load_file, load_openodd, load_transmission, Loaded, Options, Resolver, Source,
};
pub use model::{
    Attribute, ConceptKind, Condition, Module, Odd, Predicate, Probe, Section, Truth, Values,
    Verdict,
};
pub use transmission::{Origin, Transmission};
pub use value::Value;
