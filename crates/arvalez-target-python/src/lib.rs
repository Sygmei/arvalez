//! Full-featured Python SDK generator.
//!
//! All code-structure decisions live in the Tera templates.  This file only
//! provides Python-specific string helpers, Tera filters, and a
//! backward-compatible public API.  [`declare_target!`] wires the static
//! parts together.

use std::collections::HashMap;

use arvalez_target_core::to_pascal_case;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tera::Tera;

mod sanitize;
#[cfg(test)]
mod tests;

pub use arvalez_target_core::{CommonConfig, GeneratedFile};
use sanitize::is_python_keyword;
pub use sanitize::{sanitize_class_name, sanitize_identifier};

// ── Config ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TargetConfig {
    #[serde(default)]
    pub group_by_tag: bool,
}

// ── Templates ─────────────────────────────────────────────────────────────────

pub const TEMPLATES: &[(&str, &str)] = &[
    (
        "root/pyproject.toml.tera",
        include_str!("../templates/root/pyproject.toml.tera"),
    ),
    (
        "root/README.md.tera",
        include_str!("../templates/root/README.md.tera"),
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
        "root/src/{package_name}/py.typed.tera",
        include_str!("../templates/root/src/{package_name}/py.typed.tera"),
    ),
    (
        "partials/model.py.tera",
        include_str!("../templates/partials/model.py.tera"),
    ),
    (
        "partials/client_method.py.tera",
        include_str!("../templates/partials/client_method.py.tera"),
    ),
    (
        "partials/client_class.py.tera",
        include_str!("../templates/partials/client_class.py.tera"),
    ),
    (
        "partials/tag_client_class.py.tera",
        include_str!("../templates/partials/tag_client_class.py.tera"),
    ),
];

// ── Tera filters ──────────────────────────────────────────────────────────────

pub fn register_filters(tera: &mut Tera) {
    // {{ type_ref | py_type }} — TypeRef JSON → Python type annotation (models context)
    // {{ type_ref | py_type(context="client") }} — adds "models." prefix for named types
    tera.register_filter("py_type", |v: &Value, args: &HashMap<String, Value>| {
        let client_ctx = args.get("context").and_then(Value::as_str) == Some("client");
        Ok(Value::String(type_ref_to_py(v, client_ctx)))
    });

    // {{ "someIdentifier" | py_id }} — string → snake_case Python identifier
    tera.register_filter("py_id", |v: &Value, _: &HashMap<String, Value>| {
        Ok(Value::String(sanitize_identifier(v.as_str().unwrap_or(""))))
    });

    // {{ "SomeName" | py_class_name }} — string → PascalCase Python class name
    tera.register_filter("py_class_name", |v: &Value, _: &HashMap<String, Value>| {
        Ok(Value::String(sanitize_class_name(v.as_str().unwrap_or(""))))
    });

    // {{ "/users/{userId}" | py_fstring }} → "/users/{user_id}"
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
                out.push_str(&sanitize_identifier(&name));
                out.push('}');
            } else if ch == '"' {
                out.push_str("\\\"");
            } else {
                out.push(ch);
            }
        }
        Ok(Value::String(out))
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

    // {{ operations | tag_groups }} — operations[] → tag group objects
    tera.register_filter("tag_groups", |v: &Value, _: &HashMap<String, Value>| {
        let ops = v.as_array().map(Vec::as_slice).unwrap_or(&[]);
        Ok(Value::Array(compute_tag_groups(ops)))
    });

    // {{ operations | untagged_operations }} — operations without a primary tag
    tera.register_filter(
        "untagged_operations",
        |v: &Value, _: &HashMap<String, Value>| {
            let ops = v.as_array().map(Vec::as_slice).unwrap_or(&[]);
            let untagged: Vec<Value> = ops
                .iter()
                .filter(|op| operation_primary_tag(op).is_none())
                .cloned()
                .collect();
            Ok(Value::Array(untagged))
        },
    );

    // {{ op | py_doc_params }} — operation → params that carry a non-empty description
    tera.register_filter("py_doc_params", |v: &Value, _: &HashMap<String, Value>| {
        let params = v
            .get("params")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let doc_params: Vec<Value> = params
            .iter()
            .filter(|p| {
                p.get("attributes")
                    .and_then(|a| a.get("description"))
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .is_some_and(|d| !d.is_empty())
            })
            .cloned()
            .collect();
        Ok(Value::Array(doc_params))
    });

    // {{ op | py_return_type }} → {annotation, has_result, parse_expression, content_encoding}
    tera.register_filter("py_return_type", |v: &Value, _: &HashMap<String, Value>| {
        let responses = v
            .get("responses")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let success = responses.iter().find(|r| {
            r.get("status")
                .and_then(Value::as_str)
                .is_some_and(|s| s.starts_with('2'))
        });

        let result = match success {
            Some(r) => match r.get("type_ref").filter(|v| !v.is_null()) {
                Some(type_ref) => {
                    let annotation = type_ref_to_py(type_ref, true);
                    let content_encoding = r
                        .get("attributes")
                        .and_then(|a| a.get("content_encoding"))
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    serde_json::json!({
                        "annotation": annotation,
                        "has_result": true,
                        "parse_expression": annotation,
                        "content_encoding": content_encoding,
                    })
                }
                None => serde_json::json!({
                    "annotation": "None",
                    "has_result": false,
                    "parse_expression": null,
                    "content_encoding": null,
                }),
            },
            None => serde_json::json!({
                "annotation": "None",
                "has_result": false,
                "parse_expression": null,
                "content_encoding": null,
            }),
        };
        Ok(result)
    });
}

// ── TypeRef → Python type ─────────────────────────────────────────────────────

fn type_ref_to_py(v: &Value, client_context: bool) -> String {
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
        Some("named") => {
            let name = to_pascal_case(v["name"].as_str().unwrap_or("Any"));
            if client_context {
                format!("models.{name}")
            } else {
                name
            }
        }
        Some("array") => format!("list[{}]", type_ref_to_py(&v["item"], client_context)),
        Some("map") => format!("dict[str, {}]", type_ref_to_py(&v["value"], client_context)),
        Some("union") => v["variants"]
            .as_array()
            .map(|vs| {
                vs.iter()
                    .map(|v| type_ref_to_py(v, client_context))
                    .collect::<Vec<_>>()
                    .join(" | ")
            })
            .unwrap_or_else(|| "Any".into()),
        _ => "Any".into(),
    }
}

// ── Tag groups ────────────────────────────────────────────────────────────────

fn operation_primary_tag(op: &Value) -> Option<String> {
    op.get("attributes")
        .and_then(|a| a.get("tags"))
        .and_then(Value::as_array)
        .and_then(|tags| tags.first())
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_owned)
}

fn compute_tag_groups(ops: &[Value]) -> Vec<Value> {
    use std::collections::BTreeMap;
    let mut groups: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for op in ops {
        if let Some(tag) = operation_primary_tag(op) {
            groups.entry(tag).or_default().push(op.clone());
        }
    }
    groups
        .into_iter()
        .map(|(tag, group_ops)| {
            let class_base_name = sanitize_class_name(&tag);
            let property_name = sanitize_identifier(&tag);
            serde_json::json!({
                "tag": tag,
                "property_name": property_name,
                "class_base_name": class_base_name,
                "async_class_name": format!("Async{class_base_name}Api"),
                "sync_class_name": format!("Sync{class_base_name}Api"),
                "operations": group_ops,
            })
        })
        .collect()
}

// ── Declare target ─────────────────────────────────────────────────────────────

arvalez_target_core::declare_target! {
    config:    TargetConfig,
    templates: TEMPLATES,
    filters:   register_filters,
}
