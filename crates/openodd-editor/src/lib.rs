//! An editor for ASAM OpenODD YAML in the browser.
//!
//! [`doc`] is the document as the editor holds it, read from YAML and written
//! back; [`check`] runs openodd's reader on it; [`viz`] draws what the ODD
//! makes of each attribute and module; [`app`] and [`forms`] are the user
//! interface, in Leptos; [`io`] is the browser's files and storage.

pub mod app;
pub mod check;
pub mod doc;
pub mod forms;
pub mod icons;
pub mod io;
pub mod viz;
