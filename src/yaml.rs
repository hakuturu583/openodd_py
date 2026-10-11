//! YAML, read with `serde_norway` (the maintained fork of `serde_yaml`).
//!
//! It reads YAML 1.2 scalars -- only `true`/`false` are booleans, so `off` or
//! a country code `NO` stay strings -- refuses a key written twice, and keeps
//! mappings in the order written, as OpenODD needs (literals and modules are
//! read in order). This module only adds how the reference implementation
//! writes a scalar ([`NodeExt::py_str`], [`NodeExt::literal`]), since those
//! strings become bucket labels and concept names.

use serde::Deserialize;
pub use serde_norway::{Mapping, Value as Node};

use crate::error::{Error, Result};
use crate::value::{py_float_repr, Value};

/// The YAML documents in `text` (empty ones left out).
pub fn parse_all(text: &str) -> Result<Vec<Node>> {
    let mut out = Vec::new();
    for doc in serde_norway::Deserializer::from_str(text) {
        let node = Node::deserialize(doc).map_err(yaml_error)?;
        if !node.is_null() {
            out.push(node);
        }
    }
    Ok(out)
}

/// A parse error as the reference words it: a duplicate key "is written twice".
fn yaml_error(e: serde_norway::Error) -> Error {
    let message = e.to_string();
    let duplicate = regex::Regex::new(r#"duplicate entry with key "((?:[^"\\]|\\.)*)""#)
        .expect("a valid regex");
    if let Some(m) = duplicate.captures(&message) {
        let at = e
            .location()
            .map(|l| format!(" (line {}, column {})", l.line(), l.column()))
            .unwrap_or_default();
        return Error::open_odd(format!("not YAML: '{}' is written twice{at}", &m[1]));
    }
    Error::open_odd(format!("not YAML: {message}"))
}

/// The value under any tags (`!tag value` reads as `value`).
fn untag(node: &Node) -> &Node {
    match node {
        Node::Tagged(t) => untag(&t.value),
        other => other,
    }
}

/// An empty mapping.
pub fn empty_map() -> Node {
    Node::Mapping(Mapping::new())
}

pub trait NodeExt {
    /// `str(node)` as Python writes the scalar (`True` for a bool, `2.0` for a float).
    fn py_str(&self) -> String;
    /// `repr(node)`, for messages.
    fn py_repr(&self) -> String;
    /// A literal's name: a bool in lowercase, anything else as written.
    fn literal(&self) -> String;
    /// The mapping's entries, in order (none for anything else).
    fn pairs(&self) -> Vec<(&Node, &Node)>;
    /// Whether it is a mapping holding `key`.
    fn has(&self, key: &str) -> bool;
    /// Falsy as Python reads it (`None`, `0`, `""`, `[]`, `{}`).
    fn is_falsy(&self) -> bool;
    /// A scalar as a value; `None` for a list or a mapping.
    fn to_value(&self) -> Option<Value>;
    /// `float(node)`.
    fn to_f64(&self) -> Option<f64>;
    /// A list as a list, anything else as a list of itself (`null`: empty).
    fn as_list(&self) -> Vec<Node>;
    fn to_json(&self) -> serde_json::Value;
}

impl NodeExt for Node {
    fn py_str(&self) -> String {
        match untag(self) {
            Node::Null => "None".into(),
            Node::Bool(b) => if *b { "True" } else { "False" }.into(),
            Node::Number(n) => match n.as_i64() {
                Some(i) => i.to_string(),
                None => py_float_repr(n.as_f64().unwrap_or(f64::NAN)),
            },
            Node::String(s) => s.clone(),
            _ => self.py_repr(),
        }
    }

    fn py_repr(&self) -> String {
        match untag(self) {
            Node::String(s) => format!("'{s}'"),
            Node::Sequence(items) => {
                format!(
                    "[{}]",
                    items
                        .iter()
                        .map(NodeExt::py_repr)
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
            Node::Mapping(m) => format!(
                "{{{}}}",
                m.iter()
                    .map(|(k, v)| format!("{}: {}", k.py_repr(), v.py_repr()))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            other => other.py_str(),
        }
    }

    fn literal(&self) -> String {
        match untag(self) {
            Node::Bool(b) => if *b { "true" } else { "false" }.into(),
            other => other.py_str(),
        }
    }

    fn pairs(&self) -> Vec<(&Node, &Node)> {
        match untag(self) {
            Node::Mapping(m) => m.iter().collect(),
            _ => Vec::new(),
        }
    }

    fn has(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    fn is_falsy(&self) -> bool {
        match untag(self) {
            Node::Null => true,
            Node::Bool(b) => !b,
            Node::Number(n) => n.as_f64() == Some(0.0),
            Node::String(s) => s.is_empty(),
            Node::Sequence(s) => s.is_empty(),
            Node::Mapping(m) => m.is_empty(),
            Node::Tagged(_) => false,
        }
    }

    fn to_value(&self) -> Option<Value> {
        Some(match untag(self) {
            Node::Null => Value::Missing,
            Node::Bool(b) => Value::Bool(*b),
            Node::Number(n) => match n.as_i64() {
                Some(i) => Value::Int(i),
                None => Value::Float(n.as_f64()?),
            },
            Node::String(s) => Value::Str(s.clone()),
            _ => return None,
        })
    }

    fn to_f64(&self) -> Option<f64> {
        match untag(self) {
            Node::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
            Node::Number(n) => n.as_f64(),
            Node::String(s) => crate::value::parse_float(s),
            _ => None,
        }
    }

    fn as_list(&self) -> Vec<Node> {
        match untag(self) {
            Node::Null => vec![],
            Node::Sequence(items) => items.clone(),
            other => vec![other.clone()],
        }
    }

    fn to_json(&self) -> serde_json::Value {
        match untag(self) {
            Node::Mapping(m) => serde_json::Value::Object(
                m.iter().map(|(k, v)| (k.py_str(), v.to_json())).collect(),
            ),
            Node::Sequence(items) => {
                serde_json::Value::Array(items.iter().map(NodeExt::to_json).collect())
            }
            other => serde_json::to_value(other).unwrap_or(serde_json::Value::Null),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yaml_1_2_booleans_and_order() {
        let docs = parse_all("wipers: [off, on]\nb: true\na: NO\nn: 2.0\n").unwrap();
        let keys: Vec<String> = docs[0].pairs().iter().map(|(k, _)| k.py_str()).collect();
        assert_eq!(keys, ["wipers", "b", "a", "n"]);
        assert_eq!(
            docs[0]["wipers"]
                .as_list()
                .iter()
                .map(NodeExt::literal)
                .collect::<Vec<_>>(),
            ["off", "on"]
        );
        assert_eq!(docs[0]["b"].literal(), "true");
        assert_eq!(docs[0]["a"].py_str(), "NO");
        assert_eq!(docs[0]["n"].py_str(), "2.0");
    }

    #[test]
    fn a_key_written_twice_is_refused() {
        assert!(parse_all("a: 1\na: 2\n").is_err());
    }

    #[test]
    fn several_documents() {
        assert_eq!(parse_all("a: 1\n---\nb: 2\n---\n").unwrap().len(), 2);
    }
}
