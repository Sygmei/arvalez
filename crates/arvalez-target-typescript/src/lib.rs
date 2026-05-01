//! TypeScript SDK generator.
//!
//! All code-structure decisions live in the Tera templates.  Each Tera filter
//! here performs one TypeScript-specific transformation (type conversion,
//! identifier sanitisation, etc.).  [`declare_target!`] wires everything
//! together.

use std::collections::HashMap;

use arvalez_target_core::{split_words, to_pascal_case};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tera::Tera;

// ── Config ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetConfig {
    pub group_by_tag: bool,
}

impl Default for TargetConfig {
    fn default() -> Self {
        Self { group_by_tag: false }
    }
}

// ── Templates ──────────────────────────────────────────────────────────────────

/// Template name encodes the output path: strip the `root/` prefix, strip
/// `.tera`, then expand `{var}` placeholders.  Templates under `partials/` are
/// included by others and are never rendered directly to a file.
pub const TEMPLATES: &[(&str, &str)] = &[
    ("root/package.json.tera",       include_str!("../templates/root/package.json.tera")),
    ("root/tsconfig.json.tera",      include_str!("../templates/root/tsconfig.json.tera")),
    ("root/README.md.tera",          include_str!("../templates/root/README.md.tera")),
    ("root/src/models.ts.tera",      include_str!("../templates/root/src/models.ts.tera")),
    ("root/src/client.ts.tera",      include_str!("../templates/root/src/client.ts.tera")),
    ("root/src/utils.ts.tera",       include_str!("../templates/root/src/utils.ts.tera")),
    ("root/src/index.ts.tera",       include_str!("../templates/root/src/index.ts.tera")),
    ("partials/model_interface.ts.tera", include_str!("../templates/partials/model_interface.ts.tera")),
    ("partials/client_method.ts.tera",   include_str!("../templates/partials/client_method.ts.tera")),
    ("partials/tag_group.ts.tera",       include_str!("../templates/partials/tag_group.ts.tera")),
];

// ── Tera filters ───────────────────────────────────────────────────────────────

pub fn register_filters(tera: &mut Tera) {
    // {{ type_ref | ts_type }} — TypeRef JSON → TypeScript type annotation
    tera.register_filter("ts_type", |v: &Value, _: &HashMap<String, Value>| {
        Ok(Value::String(type_ref_to_ts(v)))
    });
    // {{ "someIdentifier" | ts_id }} — string → camelCase TS identifier
    tera.register_filter("ts_id", |v: &Value, _: &HashMap<String, Value>| {
        Ok(Value::String(sanitize_identifier(v.as_str().unwrap_or(""))))
    });
    // {{ "someType" | ts_type_name }} — string → PascalCase TS type name
    tera.register_filter("ts_type_name", |v: &Value, _: &HashMap<String, Value>| {
        Ok(Value::String(sanitize_type_name(v.as_str().unwrap_or(""))))
    });
    // {{ "getWidget" | ts_raw_method }} → "_getWidgetRaw"
    tera.register_filter("ts_raw_method", |v: &Value, _: &HashMap<String, Value>| {
        Ok(Value::String(format!("_{}Raw", sanitize_identifier(v.as_str().unwrap_or("")))))
    });
    // {{ "field-name" | ts_property }} — safe property key (quoted if needed)
    tera.register_filter("ts_property", |v: &Value, _: &HashMap<String, Value>| {
        let name = v.as_str().unwrap_or("");
        let result = if is_valid_ts_identifier(name) && !is_ts_keyword(name) {
            name.to_owned()
        } else {
            format!("{name:?}")
        };
        Ok(Value::String(result))
    });
    // {{ "/users/{userId}" | ts_path }} → "`/users/${userId}`"
    tera.register_filter("ts_path", |v: &Value, _: &HashMap<String, Value>| {
        Ok(Value::String(render_path_template(v.as_str().unwrap_or(""))))
    });
    // {{ "doc text" | ts_doc_text }} — escape */ in doc comments
    tera.register_filter("ts_doc_text", |v: &Value, _: &HashMap<String, Value>| {
        Ok(Value::String(v.as_str().unwrap_or("").replace("*/", "*\\/")))
    });
    // {{ "some tag" | ts_tag_prop }} — tag name → JS property identifier
    tera.register_filter("ts_tag_prop", |v: &Value, _: &HashMap<String, Value>| {
        let property = split_words(v.as_str().unwrap_or("")).join("_");
        let result = if property.is_empty() {
            "default".to_owned()
        } else if is_ts_keyword(&property) {
            format!("{property}_")
        } else {
            property
        };
        Ok(Value::String(result))
    });
    // {{ enum_values | ts_enum_expression }} — array of values → `"A" | "B"`
    tera.register_filter("ts_enum_expression", |v: &Value, _: &HashMap<String, Value>| {
        let expr = v
            .as_array()
            .map(|arr| arr.iter().map(render_enum_variant).collect::<Vec<_>>().join(" | "))
            .unwrap_or_default();
        Ok(Value::String(expr))
    });
    // {{ models | ts_client_imports }} — models array → sorted import name list
    tera.register_filter("ts_client_imports", |v: &Value, _: &HashMap<String, Value>| {
        let mut names: Vec<String> = v
            .as_array()
            .map(|arr| {
                arr.iter()
                    .map(|m| sanitize_type_name(m.get("name").and_then(Value::as_str).unwrap_or("")))
                    .collect()
            })
            .unwrap_or_default();
        names.push("JsonValue".to_owned());
        names.sort();
        names.dedup();
        Ok(Value::String(names.join(", ")))
    });
    // {{ operations | ts_tag_groups }} — operations → [{property_name, bindings}]
    tera.register_filter("ts_tag_groups", |v: &Value, _: &HashMap<String, Value>| {
        let mut groups: std::collections::BTreeMap<String, Vec<Value>> = std::collections::BTreeMap::new();
        if let Some(ops) = v.as_array() {
            for op in ops {
                let primary_tag = op
                    .get("attributes")
                    .and_then(|a| a.get("tags"))
                    .and_then(Value::as_array)
                    .and_then(|tags| tags.first())
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|t| !t.is_empty())
                    .map(ToOwned::to_owned);
                if let Some(tag) = primary_tag {
                    let method_name = sanitize_identifier(op.get("name").and_then(Value::as_str).unwrap_or(""));
                    let raw_method_name = format!("_{method_name}Raw");
                    groups.entry(tag).or_default().push(json!({
                        "method_name": method_name,
                        "raw_method_name": raw_method_name,
                    }));
                }
            }
        }
        let result: Vec<Value> = groups
            .into_iter()
            .map(|(tag, bindings)| {
                json!({ "property_name": tag_property_name(&tag), "bindings": bindings })
            })
            .collect();
        Ok(Value::Array(result))
    });
    // {{ operation | ts_args_sig }} — operation → method args signature string
    tera.register_filter("ts_args_sig", |v: &Value, _: &HashMap<String, Value>| {
        Ok(Value::String(build_args_sig(v)))
    });
    // {{ operation | ts_fwd_args }} — operation → wrapper forward-args string
    tera.register_filter("ts_fwd_args", |v: &Value, _: &HashMap<String, Value>| {
        Ok(Value::String(build_fwd_args(v)))
    });
    // {{ operation | ts_return_type }} — operation → TypeScript return type
    tera.register_filter("ts_return_type", |v: &Value, _: &HashMap<String, Value>| {
        Ok(Value::String(build_return_type(v)))
    });
    // {{ operation.request_body.media_type | ts_body_kind }} → "json"|"form"|"binary"
    tera.register_filter("ts_body_kind", |v: &Value, _: &HashMap<String, Value>| {
        let kind = match v.as_str().unwrap_or("") {
            mt if mt == "application/json" => "json",
            mt if mt.starts_with("multipart/form-data") => "form",
            _ => "binary",
        };
        Ok(Value::String(kind.to_owned()))
    });
    // {{ operation | ts_response_encoding }} → content_encoding string or null
    tera.register_filter("ts_response_encoding", |v: &Value, _: &HashMap<String, Value>| {
        let encoding = v
            .get("responses")
            .and_then(Value::as_array)
            .and_then(|rs| {
                rs.iter()
                    .find(|r| r.get("status").and_then(Value::as_str).is_some_and(|s| s.starts_with('2')))
            })
            .and_then(|r| r.get("attributes"))
            .and_then(|a| a.get("content_encoding"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        Ok(encoding.map(Value::String).unwrap_or(Value::Null))
    });
    // {{ operation.params | ts_doc_params }} — filter params that have a description
    tera.register_filter("ts_doc_params", |v: &Value, _: &HashMap<String, Value>| {
        let doc_params: Vec<Value> = v
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|p| {
                        let desc = p
                            .get("attributes")
                            .and_then(|a| a.get("description"))
                            .and_then(Value::as_str)
                            .map(str::trim)
                            .filter(|d| !d.is_empty())?;
                        let name = sanitize_identifier(p.get("name").and_then(Value::as_str).unwrap_or(""));
                        Some(json!({ "name": name, "description": desc.replace("*/", "*\\/") }))
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(Value::Array(doc_params))
    });
}

// ── TypeRef → TypeScript type ──────────────────────────────────────────────────

fn type_ref_to_ts(v: &Value) -> String {
    match v.get("kind").and_then(Value::as_str) {
        Some("primitive") => match v["name"].as_str().unwrap_or("unknown") {
            "string" => "string",
            "integer" | "number" => "number",
            "boolean" => "boolean",
            "binary" => "Blob",
            "null" => "null",
            "any" | "object" => "JsonValue",
            _ => "unknown",
        }
        .into(),
        Some("named") => sanitize_type_name(v["name"].as_str().unwrap_or("")),
        Some("array") => format!("{}[]", type_ref_to_ts(&v["item"])),
        Some("map") => format!("Record<string, {}>", type_ref_to_ts(&v["value"])),
        Some("union") => v["variants"]
            .as_array()
            .map(|vs| vs.iter().map(type_ref_to_ts).collect::<Vec<_>>().join(" | "))
            .unwrap_or_else(|| "unknown".into()),
        _ => "unknown".into(),
    }
}

// ── String / identifier helpers ────────────────────────────────────────────────

fn sanitize_type_name(name: &str) -> String {
    let out = to_pascal_case(name);
    if out.is_empty() { "GeneratedModel".into() } else { out }
}

fn sanitize_identifier(name: &str) -> String {
    let words = split_words(name);
    let mut candidate = if words.is_empty() {
        "value".into()
    } else {
        let mut iter = words.into_iter();
        let mut result = iter.next().unwrap_or_else(|| "value".into());
        for part in iter {
            let mut chars = part.chars();
            if let Some(first) = chars.next() {
                result.extend(first.to_uppercase());
                result.push_str(chars.as_str());
            }
        }
        result
    };
    // TypeScript identifiers cannot start with a digit.
    if candidate.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        candidate.insert(0, '_');
    }
    if is_ts_keyword(&candidate) {
        candidate.push('_');
    }
    candidate
}

fn tag_property_name(name: &str) -> String {
    let property = split_words(name).join("_");
    if property.is_empty() {
        "default".into()
    } else if is_ts_keyword(&property) {
        format!("{property}_")
    } else {
        property
    }
}

fn render_path_template(path: &str) -> String {
    let mut result = String::from("`");
    let mut chars = path.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '{' => {
                let mut name = String::new();
                for nc in chars.by_ref() {
                    if nc == '}' { break; }
                    name.push(nc);
                }
                result.push_str("${");
                result.push_str(&sanitize_identifier(&name));
                result.push('}');
            }
            '`' => result.push_str("\\`"),
            _ => result.push(ch),
        }
    }
    result.push('`');
    result
}

fn render_enum_variant(value: &Value) -> String {
    match value {
        Value::String(s) => format!("{s:?}"),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::Null => "null".into(),
        _ => "unknown".into(),
    }
}

fn is_valid_ts_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    match chars.next() {
        Some(c) if c == '_' || c == '$' || c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c == '_' || c == '$' || c.is_ascii_alphanumeric())
}

fn is_ts_keyword(value: &str) -> bool {
    matches!(
        value,
        "break" | "case" | "catch" | "class" | "const" | "continue" | "debugger" | "default"
        | "delete" | "do" | "else" | "enum" | "export" | "extends" | "false" | "finally"
        | "for" | "function" | "if" | "import" | "in" | "instanceof" | "new" | "null"
        | "return" | "super" | "switch" | "this" | "throw" | "true" | "try" | "typeof"
        | "var" | "void" | "while" | "with" | "yield"
    )
}

// ── Operation helpers ──────────────────────────────────────────────────────────

fn op_params(op: &Value) -> &[Value] {
    op.get("params").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[])
}

fn param_type(param: &Value) -> String {
    param.get("type_ref").map(|tr| type_ref_to_ts(tr)).unwrap_or_else(|| "unknown".into())
}

fn build_args_sig(op: &Value) -> String {
    let params = op_params(op);
    let body = op.get("request_body");
    let mut args: Vec<String> = Vec::new();
    for p in params.iter().filter(|p| p.get("required").and_then(Value::as_bool).unwrap_or(false)) {
        args.push(format!("{}: {}", sanitize_identifier(p.get("name").and_then(Value::as_str).unwrap_or("")), param_type(p)));
    }
    if let Some(rb) = body {
        if rb.get("required").and_then(Value::as_bool).unwrap_or(false) {
            let ty = rb.get("type_ref").map(|tr| type_ref_to_ts(tr)).unwrap_or_else(|| "unknown".into());
            args.push(format!("body: {ty}"));
        }
    }
    for p in params.iter().filter(|p| !p.get("required").and_then(Value::as_bool).unwrap_or(false)) {
        args.push(format!("{}?: {}", sanitize_identifier(p.get("name").and_then(Value::as_str).unwrap_or("")), param_type(p)));
    }
    if let Some(rb) = body {
        if !rb.get("required").and_then(Value::as_bool).unwrap_or(false) {
            let ty = rb.get("type_ref").map(|tr| type_ref_to_ts(tr)).unwrap_or_else(|| "unknown".into());
            args.push(format!("body?: {ty}"));
        }
    }
    args.push("requestOptions?: RequestOptions".into());
    args.join(", ")
}

fn build_fwd_args(op: &Value) -> String {
    let params = op_params(op);
    let body = op.get("request_body");
    let mut args: Vec<String> = Vec::new();
    for p in params.iter().filter(|p| p.get("required").and_then(Value::as_bool).unwrap_or(false)) {
        args.push(sanitize_identifier(p.get("name").and_then(Value::as_str).unwrap_or("")));
    }
    if let Some(rb) = body {
        if rb.get("required").and_then(Value::as_bool).unwrap_or(false) {
            args.push("body".into());
        }
    }
    for p in params.iter().filter(|p| !p.get("required").and_then(Value::as_bool).unwrap_or(false)) {
        args.push(sanitize_identifier(p.get("name").and_then(Value::as_str).unwrap_or("")));
    }
    if let Some(rb) = body {
        if !rb.get("required").and_then(Value::as_bool).unwrap_or(false) {
            args.push("body".into());
        }
    }
    args.push("requestOptions".into());
    args.join(", ")
}

fn build_return_type(op: &Value) -> String {
    op.get("responses")
        .and_then(Value::as_array)
        .and_then(|rs| {
            rs.iter()
                .find(|r| r.get("status").and_then(Value::as_str).is_some_and(|s| s.starts_with('2')))
        })
        .and_then(|r| r.get("type_ref"))
        .map(|tr| type_ref_to_ts(tr))
        .unwrap_or_else(|| "void".into())
}

// ── Target wiring ──────────────────────────────────────────────────────────────

arvalez_target_core::declare_target! {
    config:    TargetConfig,
    templates: TEMPLATES,
    filters:   register_filters,
}

#[cfg(test)]
mod tests;
