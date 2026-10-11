//! Values an attribute takes, and how they are labelled.
//!
//! Labels are the bucket names written to coverage files and compared by the
//! conditions, so they follow the reference implementation exactly: a bool is
//! `true` / `false`, an integer its digits, a float Python's `repr` (`30.0`,
//! `1e-05`), and a numeric bucket edge `%.12g`.

use std::fmt;

/// A value of an attribute, as a probe reads it or a condition names it.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// The probe could not read it: OpenODD's missing value.
    Missing,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    /// Present, but could be anything: what deciding the buckets outside the
    /// ODD leaves the other attributes at, and what a map-only reader returns
    /// for a value only a run knows.
    Open,
    /// Every value of one bucket of the attribute's cover item.
    Bucket(usize),
}

impl Value {
    /// The value as a number when it is one (not a bool).
    pub fn number(&self) -> Option<f64> {
        match self {
            Value::Int(i) => Some(*i as f64),
            Value::Float(x) => Some(*x),
            _ => None,
        }
    }

    /// Whether the value says nothing: missing, or a NaN.
    pub fn is_missing(&self) -> bool {
        match self {
            Value::Missing => true,
            Value::Float(x) => x.is_nan(),
            _ => false,
        }
    }

    /// The value as a float the way `float(value)` reads it: a bool is 0 or 1,
    /// a string is parsed.
    pub fn as_float(&self) -> Option<f64> {
        match self {
            Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
            Value::Int(i) => Some(*i as f64),
            Value::Float(x) => Some(*x),
            Value::Str(s) => parse_float(s),
            _ => None,
        }
    }

    /// The bucket label of a categorical value: a bool in lowercase, a number
    /// as Python writes it, a string as it is.
    pub fn label(&self) -> String {
        match self {
            Value::Bool(b) => if *b { "true" } else { "false" }.to_string(),
            Value::Int(i) => i.to_string(),
            Value::Float(x) => py_float_repr(*x),
            Value::Str(s) => s.clone(),
            Value::Missing => "None".to_string(),
            Value::Open => "UNDECIDED".to_string(),
            Value::Bucket(i) => format!("<bucket {i}>"),
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.label())
    }
}

/// `float(text)` as Python reads it (surrounding blanks, `inf`, `nan`, `_`).
pub fn parse_float(text: &str) -> Option<f64> {
    let t = text.trim();
    if t.is_empty() || t.starts_with('_') || t.ends_with('_') || t.contains("__") {
        return None;
    }
    let cleaned: String = t.chars().filter(|c| *c != '_').collect();
    let lower = cleaned.to_lowercase();
    let (sign, body) = match lower.strip_prefix('-') {
        Some(rest) => (-1.0, rest),
        None => (1.0, lower.strip_prefix('+').unwrap_or(&lower)),
    };
    match body {
        "inf" | "infinity" => return Some(sign * f64::INFINITY),
        "nan" => return Some(f64::NAN),
        _ => {}
    }
    if body
        .chars()
        .any(|c| !(c.is_ascii_digit() || matches!(c, '.' | 'e' | '+' | '-')))
    {
        return None;
    }
    cleaned.parse::<f64>().ok()
}

/// Python's `repr(float)`: the shortest digits that read back the same, in
/// fixed notation for exponents from -4 to 15, else in scientific notation.
pub fn py_float_repr(x: f64) -> String {
    if x.is_nan() {
        return "nan".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "inf".into() } else { "-inf".into() };
    }
    if x == 0.0 {
        return if x.is_sign_negative() {
            "-0.0".into()
        } else {
            "0.0".into()
        };
    }
    // Rust's `{:e}` is the shortest round-trip mantissa: "1.5e-5", "1e16".
    let sci = format!("{x:e}");
    let (mantissa, exp) = sci.split_once('e').expect("{:e} has an exponent");
    let exp: i32 = exp.parse().expect("an integer exponent");
    let negative = mantissa.starts_with('-');
    let digits: String = mantissa.chars().filter(|c| c.is_ascii_digit()).collect();
    let sign = if negative { "-" } else { "" };
    if (-4..16).contains(&exp) {
        let point = exp + 1; // digits before the decimal point
        let s = if point <= 0 {
            format!("0.{}{}", "0".repeat((-point) as usize), digits)
        } else if point as usize >= digits.len() {
            format!("{}{}.0", digits, "0".repeat(point as usize - digits.len()))
        } else {
            let (a, b) = digits.split_at(point as usize);
            format!("{a}.{b}")
        };
        format!("{sign}{s}")
    } else {
        let (first, rest) = digits.split_at(1);
        let m = if rest.is_empty() {
            first.to_string()
        } else {
            format!("{first}.{rest}")
        };
        format!(
            "{sign}{m}e{}{:02}",
            if exp < 0 { '-' } else { '+' },
            exp.abs()
        )
    }
}

/// C's `%.{precision}g`: `precision` significant digits, trailing zeros dropped.
pub fn format_g(x: f64, precision: usize) -> String {
    if x.is_nan() {
        return "nan".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "inf".into() } else { "-inf".into() };
    }
    if x == 0.0 {
        return if x.is_sign_negative() {
            "-0".into()
        } else {
            "0".into()
        };
    }
    let p = precision.max(1);
    let sci = format!("{:.*e}", p - 1, x);
    let (mantissa, exp) = sci.split_once('e').expect("an exponent");
    let exp: i32 = exp.parse().expect("an integer exponent");
    if exp < -4 || exp >= p as i32 {
        let m = trim_zeros(mantissa);
        format!("{m}e{}{:02}", if exp < 0 { '-' } else { '+' }, exp.abs())
    } else {
        let decimals = (p as i32 - 1 - exp).max(0) as usize;
        trim_zeros(&format!("{:.*}", decimals, x))
    }
}

fn trim_zeros(s: &str) -> String {
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floats_print_as_python_does() {
        for (x, want) in [
            (30.0, "30.0"),
            (0.1, "0.1"),
            (1e16, "1e+16"),
            (1e15, "1000000000000000.0"),
            (1e-5, "1e-05"),
            (0.0001, "0.0001"),
            (-2.5, "-2.5"),
            (123456.789, "123456.789"),
        ] {
            assert_eq!(py_float_repr(x), want, "{x}");
        }
    }

    #[test]
    fn edges_print_as_percent_g() {
        for (x, want) in [
            (0.0, "0"),
            (30.0, "30"),
            (0.5, "0.5"),
            (1.0 / 3.0, "0.333333333333"),
            (1e13, "1e+13"),
            (-15.0, "-15"),
            (123456789012.0, "123456789012"),
        ] {
            assert_eq!(format_g(x, 12), want, "{x}");
        }
    }

    #[test]
    fn strings_read_as_float_does() {
        assert_eq!(parse_float(" 3.5 "), Some(3.5));
        assert_eq!(parse_float("1_000"), Some(1000.0));
        assert_eq!(parse_float("-inf"), Some(f64::NEG_INFINITY));
        assert_eq!(parse_float("heavy"), None);
    }
}
