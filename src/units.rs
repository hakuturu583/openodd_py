//! Units an ODD condition may be written in, and conversion between them.
//!
//! An attribute's probe returns its value in one unit (`km/h` for a speed),
//! and an OpenODD condition may name another (`"<= 30 m/s"`). The condition's
//! number is converted into the probe's unit once, when the ODD is loaded.
//!
//! Units belong to OpenODD's *unit types* (`velocity`, `length`,
//! `precipitation_rate`, ...). A unit converts to its type's base unit by
//! `value * scale + offset`, which covers temperatures as well. A taxonomy can
//! add units with OpenODD's `conversion:` block ([`Units::add_conversions`]).

use indexmap::IndexMap;

use crate::error::{Error, Result};

/// (unit type, scale, offset) to the unit type's base unit.
type Entry = (String, f64, f64);

const BUILTIN: &[(&str, &str, f64, f64)] = &[
    // length (m)
    ("m", "length", 1.0, 0.0),
    ("km", "length", 1000.0, 0.0),
    ("cm", "length", 0.01, 0.0),
    ("mm", "length", 0.001, 0.0),
    ("mi", "length", 1609.344, 0.0),
    ("in", "length", 0.0254, 0.0),
    ("ft", "length", 0.3048, 0.0),
    // velocity (m/s)
    ("m/s", "velocity", 1.0, 0.0),
    ("km/h", "velocity", 1.0 / 3.6, 0.0),
    ("mph", "velocity", 0.44704, 0.0),
    // acceleration (m/s^2)
    ("m/s^2", "acceleration", 1.0, 0.0),
    ("g", "acceleration", 9.80665, 0.0),
    // time (s)
    ("s", "time", 1.0, 0.0),
    ("ms", "time", 0.001, 0.0),
    ("min", "time", 60.0, 0.0),
    ("h", "time", 3600.0, 0.0),
    // angle (deg)
    ("deg", "angle", 1.0, 0.0),
    ("rad", "angle", 57.29577951308232, 0.0),
    // precipitation rate (mm/h)
    ("mm/h", "precipitation_rate", 1.0, 0.0),
    // temperature (C)
    ("C", "temperature", 1.0, 0.0),
    ("K", "temperature", 1.0, -273.15),
    ("F", "temperature", 5.0 / 9.0, -32.0 * 5.0 / 9.0),
    // fraction (1); % is fraction * 100
    ("%", "fraction", 0.01, 0.0),
    // bandwidth (bit/s)
    ("bps", "bandwidth", 1.0, 0.0),
    ("kbps", "bandwidth", 1e3, 0.0),
    ("Mbps", "bandwidth", 1e6, 0.0),
    ("Gbps", "bandwidth", 1e9, 0.0),
    // frequency (Hz)
    ("Hz", "frequency", 1.0, 0.0),
    ("kHz", "frequency", 1e3, 0.0),
    // illuminance (lx)
    ("lx", "illuminance", 1.0, 0.0),
    // count and occurrence
    ("count", "count", 1.0, 0.0),
    ("1/h", "occurrence", 1.0, 0.0),
    ("1/hr", "occurrence", 1.0, 0.0),
    ("occ/hr", "occurrence", 1.0, 0.0),
];

const ALIASES: &[(&str, &str)] = &[
    ("kph", "km/h"),
    ("kmh", "km/h"),
    ("kmph", "km/h"),
    ("mps", "m/s"),
    ("msec", "ms"),
    ("sec", "s"),
    ("hr", "h"),
    ("mm/hr", "mm/h"),
    ("degree", "deg"),
    ("degrees", "deg"),
    ("percent", "%"),
];

/// Unit types that are the same dimension under another name.
fn same_type(name: &str) -> &str {
    match name {
        "duration" => "time",
        "distance" => "length",
        "speed" => "velocity",
        other => other,
    }
}

/// The canonical spelling of `unit` (`kph` -> `km/h`); `""` for none.
pub fn normalize_unit(unit: &str) -> String {
    let u = unit.trim();
    let lower = u.to_lowercase();
    for (alias, canonical) in ALIASES {
        if *alias == lower || *alias == u {
            return (*canonical).to_string();
        }
    }
    u.to_string()
}

/// A table of units: the built-in ones, and those a taxonomy adds.
#[derive(Debug, Clone)]
pub struct Units {
    table: IndexMap<String, Entry>,
}

impl Default for Units {
    fn default() -> Self {
        Self::new()
    }
}

/// One `conversion:` entry: a value in `from` is `value * scale + offset` in `to`.
#[derive(Debug, Clone, PartialEq)]
pub struct Conversion {
    pub unit_type: String,
    pub from: String,
    pub to: String,
    pub scale: f64,
    pub offset: f64,
}

impl Units {
    pub fn new() -> Self {
        let table = BUILTIN
            .iter()
            .map(|(u, t, s, o)| ((*u).to_string(), ((*t).to_string(), *s, *o)))
            .collect();
        Units { table }
    }

    /// Add the units of an OpenODD `conversion:` block, in the order written.
    ///
    /// A unit known already anchors its type, so the others join it.
    pub fn add_conversions(&mut self, conversions: &[Conversion]) {
        for c in conversions {
            let (src, dst) = (normalize_unit(&c.from), normalize_unit(&c.to));
            if !self.table.contains_key(&src) && !self.table.contains_key(&dst) {
                // Neither is known: make `from` the base of the type.
                self.table
                    .insert(src.clone(), (same_type(&c.unit_type).to_string(), 1.0, 0.0));
            }
            if self.table.contains_key(&src) && !self.table.contains_key(&dst) {
                let (t, s_scale, s_offset) = self.table[&src].clone();
                // dst = src * scale + offset  =>  src = (dst - offset) / scale
                self.table.insert(
                    dst,
                    (
                        t,
                        s_scale / c.scale,
                        s_offset - s_scale * c.offset / c.scale,
                    ),
                );
            } else if self.table.contains_key(&dst) && !self.table.contains_key(&src) {
                let (t, d_scale, d_offset) = self.table[&dst].clone();
                self.table
                    .insert(src, (t, d_scale * c.scale, d_scale * c.offset + d_offset));
            }
        }
    }

    /// The unit type `unit` measures, or `None` if it is unknown.
    pub fn unit_type(&self, unit: &str) -> Option<&str> {
        self.table.get(&normalize_unit(unit)).map(|e| e.0.as_str())
    }

    /// `value` in `from_unit`, expressed in `to_unit`.
    ///
    /// An empty unit on either side means the value is already in the other's
    /// unit, and so does the same unit on both sides, known or not.
    pub fn convert(&self, value: f64, from_unit: &str, to_unit: &str) -> Result<f64> {
        let (src, dst) = (normalize_unit(from_unit), normalize_unit(to_unit));
        if src.is_empty() || dst.is_empty() || src == dst {
            return Ok(value);
        }
        let s = self
            .table
            .get(&src)
            .ok_or_else(|| Error::Unit(format!("unknown unit '{from_unit}'")))?;
        let d = self
            .table
            .get(&dst)
            .ok_or_else(|| Error::Unit(format!("unknown unit '{to_unit}'")))?;
        if s.0 != d.0 {
            return Err(Error::Unit(format!(
                "cannot convert '{from_unit}' ({}) into '{to_unit}' ({})",
                s.0, d.0
            )));
        }
        let base = value * s.1 + s.2;
        Ok((base - d.2) / d.1)
    }

    /// Refuse `unit` when it does not measure `unit_type` (both known).
    pub fn check_type(&self, unit: &str, unit_type: &str) -> Result<()> {
        let expected = same_type(unit_type);
        let known_type = self.table.values().any(|e| e.0 == expected);
        if let Some(actual) = self.unit_type(unit) {
            if known_type && actual != expected {
                return Err(Error::Unit(format!(
                    "'{unit}' measures {actual}, not {unit_type}"
                )));
            }
        }
        Ok(())
    }
}

/// `value` in `from_unit`, expressed in `to_unit`, with the built-in units.
pub fn convert(value: f64, from_unit: &str, to_unit: &str) -> Result<f64> {
    Units::new().convert(value, from_unit, to_unit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_within_a_type() {
        assert!((convert(36.0, "km/h", "m/s").unwrap() - 10.0).abs() < 1e-12);
        assert!((convert(0.0, "C", "F").unwrap() - 32.0).abs() < 1e-9);
        assert_eq!(convert(5.0, "", "km/h").unwrap(), 5.0);
        assert_eq!(normalize_unit("KPH"), "km/h");
    }

    #[test]
    fn refuses_across_types() {
        assert!(convert(1.0, "m", "s").is_err());
        assert!(convert(1.0, "furlong", "m").is_err());
    }

    #[test]
    fn a_conversion_block_adds_units() {
        let mut u = Units::new();
        u.add_conversions(&[Conversion {
            unit_type: "length".into(),
            from: "m".into(),
            to: "yd".into(),
            scale: 1.0936133,
            offset: 0.0,
        }]);
        assert!((u.convert(1.0, "m", "yd").unwrap() - 1.0936133).abs() < 1e-9);
    }
}
