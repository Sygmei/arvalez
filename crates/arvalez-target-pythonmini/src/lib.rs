//! Minimal single-file Python SDK generator.
//!
//! The only Rust logic here is Python-specific string sanitisation and
//! TypeRef → Python type conversion.  All code-structure decisions live in
//! the Tera templates.  [`declare_target!`] wires the static parts together.

use std::collections::HashMap;

use arvalez_target_core::{operation_with_identifiers, to_pascal_case, to_snake_identifier};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tera::Tera;

#[cfg(test)]
mod tests;

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
    tera.register_filter("pymini_operation", |v: &Value, _: &HashMap<String, Value>| {
        Ok(operation_with_identifiers(
            v,
            &["self", "body", "url", "params", "headers", "response"],
            |name| {
                let mut identifier = to_snake_identifier(name);
                if is_python_keyword(&identifier) {
                    identifier.push('_');
                }
                identifier
            },
        ))
    });
    // {{ type_ref | py_type }} — TypeRef JSON → Python type annotation
    tera.register_filter("py_type", |v: &Value, _: &HashMap<String, Value>| {
        Ok(Value::String(type_ref_to_py(v)))
    });
    // {{ field | py_field_assignment(py_name=py_name) }} - field to default/alias suffix
    tera.register_filter(
        "py_field_assignment",
        |v: &Value, args: &HashMap<String, Value>| {
            let field_name = v.get("name").and_then(Value::as_str).unwrap_or("");
            let py_name = args
                .get("py_name")
                .and_then(Value::as_str)
                .unwrap_or(field_name);
            let optional = v.get("optional").and_then(Value::as_bool).unwrap_or(false);
            let default = v
                .get("attributes")
                .and_then(Value::as_object)
                .and_then(|attributes| attributes.get("default"));

            let default_expr = default
                .map(py_literal)
                .or_else(|| optional.then(|| "None".to_owned()));
            let needs_alias = field_name != py_name;

            if needs_alias {
                let mut field_args = Vec::new();
                if let Some(default_expr) = default_expr {
                    field_args.push(format!("default={default_expr}"));
                }
                field_args.push(format!(
                    "alias={}",
                    py_literal(&Value::String(field_name.to_owned()))
                ));
                return Ok(Value::String(format!(
                    " = Field({})",
                    field_args.join(", ")
                )));
            }

            Ok(Value::String(
                default_expr
                    .map(|expr| format!(" = {expr}"))
                    .unwrap_or_default(),
            ))
        },
    );
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

fn py_literal(value: &Value) -> String {
    match value {
        Value::Null => "None".to_owned(),
        Value::Bool(value) => {
            if *value {
                "True".to_owned()
            } else {
                "False".to_owned()
            }
        }
        Value::Number(value) => value.to_string(),
        Value::String(value) => serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_owned()),
        Value::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(py_literal)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Object(values) => format!(
            "{{{}}}",
            values
                .iter()
                .map(|(key, value)| format!(
                    "{}: {}",
                    py_literal(&Value::String(key.clone())),
                    py_literal(value)
                ))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
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
        Some("enum") => type_ref_to_py(&v["base"]),
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
