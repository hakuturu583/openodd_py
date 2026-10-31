//! An editor for ASAM OpenODD YAML in the browser.
//!
//! [`doc`] is the document as the editor holds it, read from YAML and written
//! back; [`check`] runs openodd's reader on it; [`viz`] draws what the ODD
//! makes of each attribute and module; [`app`] and [`forms`] are the user
//! interface, in Leptos; [`io`] is the browser's files and storage.
//!
//! The rules editor edits a backend's fuzzy rules: [`rules_doc`] is the rule
//! file as it holds it, and [`rules_app`] its interface.

pub mod app;
pub mod binding;
pub mod binding_app;
pub mod check;
pub mod doc;
pub mod forms;
pub mod icons;
pub mod io;
pub mod rules_app;
pub mod rules_doc;
pub mod viz;
