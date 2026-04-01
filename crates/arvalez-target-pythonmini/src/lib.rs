//! Minimal single-file Python SDK generator.
//!
//! The only Rust logic here is Python-specific string sanitisation and
//! TypeRef → Python type conversion.  All code-structure decisions live in
//! the Tera templates.  [`declare_target!`] wires the static parts together.

use std::collections::HashMap;

use arvalez_target_core::{to_pascal_case, to_snake_identifier};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tera::Tera;

// ── Config ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetConfig {}

// ── Generator ─────────────────────────────────────────────────────────────────

/// Template name encodes the output path: strip the `root/` prefix, strip `.tera`,
/// then expand `{var}` placeholders.  Templates under `partials/` are included by
/// others and are never rendered directly to a file.
pub const TEMPLATES: &[(&str, &str)] = &[
    (
        "root/pyproject.toml.tera",
        include_str!("../templates/root/pyproject.toml.tera"),
    ),
    (
        "root/src/{package_name}/__init__.py.tera",
        include_str!("../templates/root/src/{package_name}/__init__.py.tera"),
    ),
    (
        "root/src/{package_name}/models.py.tera",
        include_str!("../templates/root/src/{package_name}/models.py.tera"),
    ),
    (
        "root/src/{package_name}/client.py.tera",
        include_str!("../templates/root/src/{package_name}/client.py.tera"),
    ),
    (
        "partials/model.py.tera",
        include_str!("../templates/partials/model.py.tera"),
    ),
];

// ── Tera filters ──────────────────────────────────────────────────────────────

pub fn register_filters(tera: &mut Tera) {
    // {{ type_ref | py_type }} — TypeRef JSON → Python type annotation
    tera.register_filter("py_type", |v: &Value, _: &HashMap<String, Value>| {
        Ok(Value::String(type_ref_to_py(v)))
    });
    // {{ "someIdentifier" | py_id }} — string → snake_case Python identifier (digit-safe + keyword-escaped)
    tera.register_filter("py_id", |v: &Value, _: &HashMap<String, Value>| {
        let mut s = to_snake_identifier(v.as_str().unwrap_or(""));
        if is_python_keyword(&s) {
            s.push('_');
        }
        Ok(Value::String(s))
    });
    // {{ "import" | suffix_with_underscore_if_keyword }} → "import_"
    tera.register_filter(
        "suffix_with_underscore_if_keyword",
        |v: &Value, _: &HashMap<String, Value>| {
            let s = v.as_str().unwrap_or("");
            Ok(Value::String(if is_python_keyword(s) {
                format!("{s}_")
            } else {
                s.to_string()
            }))
        },
    );
    // {{ "/users/{userId}" | py_fstring }} → "/users/{user_id}" (sanitises param names)
    tera.register_filter("py_fstring", |v: &Value, _: &HashMap<String, Value>| {
        let path = v.as_str().unwrap_or("");
        let mut out = String::with_capacity(path.len());
        let mut chars = path.chars().peekable();
        while let Some(ch) = chars.next() {
            if ch == '{' {
                let mut name = String::new();
                for nc in chars.by_ref() {
                    if nc == '}' {
                        break;
                    }
                    name.push(nc);
                }
                out.push('{');
                out.push_str(&to_snake_identifier(&name));
                out.push('}');
            } else {
                out.push(ch);
            }
        }
        Ok(Value::String(out))
    });
}

// ── TypeRef → Python type ─────────────────────────────────────────────────────

fn type_ref_to_py(v: &Value) -> String {
    match v.get("kind").and_then(Value::as_str) {
        Some("primitive") => match v["name"].as_str().unwrap_or("any") {
            "string" => "str",
            "integer" => "int",
            "number" => "float",
            "boolean" => "bool",
            "binary" => "bytes",
            "null" => "None",
            _ => "Any",
        }
        .into(),
        Some("named") => to_pascal_case(v["name"].as_str().unwrap_or("Any")),
        Some("array") => format!("list[{}]", type_ref_to_py(&v["item"])),
        Some("map") => format!("dict[str, {}]", type_ref_to_py(&v["value"])),
        Some("union") => v["variants"]
            .as_array()
            .map(|vs| {
                vs.iter()
                    .map(type_ref_to_py)
                    .collect::<Vec<_>>()
                    .join(" | ")
            })
            .unwrap_or_else(|| "Any".into()),
        _ => "Any".into(),
    }
}

// ── String helpers ─────────────────────────────────────────────────────────────

const PYTHON_KEYWORDS: &[&str] = &[
    "False", "None", "True", "and", "as", "assert", "async", "await", "break", "class", "continue",
    "def", "del", "elif", "else", "except", "finally", "for", "from", "global", "if", "import",
    "in", "is", "lambda", "nonlocal", "not", "or", "pass", "raise", "return", "try", "while",
    "with", "yield", "type", "match", "case",
];

fn is_python_keyword(s: &str) -> bool {
    PYTHON_KEYWORDS.contains(&s)
}

arvalez_target_core::declare_target! {
    config:    TargetConfig,
    templates: TEMPLATES,
    filters:   register_filters,
}
