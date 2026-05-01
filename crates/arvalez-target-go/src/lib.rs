//! Minimal single-file Go SDK generator.
//!
//! The only Rust logic here is Go-specific type conversion, identifier
//! sanitisation, and operation helpers exposed as Tera filters.  All
//! code-structure decisions live in the Tera templates.
//! [`declare_target!`] wires the static parts together.

mod sanitize;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, HashMap};

use arvalez_target_core::CommonConfig;
pub use arvalez_target_core::GeneratedFile;
use arvalez_ir::CoreIr;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tera::Tera;

use sanitize::{sanitize_exported_identifier, sanitize_identifier, sanitize_package_name};

// ── Config ────────────────────────────────────────────────────────────────────

/// Configuration for the Go SDK generator.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetConfig {
    /// Go module path, e.g. `"github.com/acme/client"`.
    #[serde(default = "default_module_path")]
    pub module_path: String,
    /// When `true`, operations are grouped by their primary tag into service structs.
    #[serde(default)]
    pub group_by_tag: bool,
}

impl Default for TargetConfig {
    fn default() -> Self {
        Self { module_path: default_module_path(), group_by_tag: false }
    }
}

fn default_module_path() -> String {
    "github.com/arvalez/client".into()
}

// ── Templates ─────────────────────────────────────────────────────────────────

pub const TEMPLATES: &[(&str, &str)] = &[
    ("root/go.mod.tera",                         include_str!("../templates/root/go.mod.tera")),
    ("root/README.md.tera",                      include_str!("../templates/root/README.md.tera")),
    ("root/models.go.tera",                      include_str!("../templates/root/models.go.tera")),
    ("root/client.go.tera",                      include_str!("../templates/root/client.go.tera")),
    ("root/utils.go.tera",                       include_str!("../templates/root/utils.go.tera")),
    ("partials/model_struct.go.tera",            include_str!("../templates/partials/model_struct.go.tera")),
    ("partials/service.go.tera",                 include_str!("../templates/partials/service.go.tera")),
    ("partials/client_method.go.tera",           include_str!("../templates/partials/client_method.go.tera")),
];

// ── Tera filters ──────────────────────────────────────────────────────────────

pub fn register_filters(tera: &mut Tera) {
    // {{ "get_widget" | go_exported }} → "GetWidget"
    tera.register_filter("go_exported", |v: &Value, _: &HashMap<String, Value>| {
        Ok(Value::String(sanitize_exported_identifier(v.as_str().unwrap_or(""))))
    });
    // {{ "widget_id" | go_id }} → "widgetId"
    tera.register_filter("go_id", |v: &Value, _: &HashMap<String, Value>| {
        Ok(Value::String(sanitize_identifier(v.as_str().unwrap_or(""))))
    });
    // {{ "github.com/acme/client" | go_pkg }} → "client"
    tera.register_filter("go_pkg", |v: &Value, _: &HashMap<String, Value>| {
        let module = v.as_str().unwrap_or("");
        let segment = module.rsplit('/').next().unwrap_or(module);
        Ok(Value::String(sanitize_package_name(segment)))
    });
    // {{ type_ref | go_type }} → Go type string
    tera.register_filter("go_type", |v: &Value, _: &HashMap<String, Value>| {
        Ok(Value::String(go_type_from_value(v)))
    });
    // {{ field | go_field_type }} → Go field type handling optional/nullable → pointer
    tera.register_filter("go_field_type", |v: &Value, _: &HashMap<String, Value>| {
        let optional = v.get("optional").and_then(Value::as_bool).unwrap_or(false);
        let nullable = v.get("nullable").and_then(Value::as_bool).unwrap_or(false);
        let type_ref = v.get("type_ref").cloned().unwrap_or(Value::Null);
        Ok(Value::String(go_field_type_from_value(&type_ref, optional, nullable)))
    });
    // {{ field | go_json_tag }} → `json:"name"` or `json:"name,omitempty"`
    tera.register_filter("go_json_tag", |v: &Value, _: &HashMap<String, Value>| {
        let name = v.get("name").and_then(Value::as_str).unwrap_or("");
        let optional = v.get("optional").and_then(Value::as_bool).unwrap_or(false);
        let tag = if optional { format!("`json:\"{name},omitempty\"`") } else { format!("`json:\"{name}\"`") };
        Ok(Value::String(tag))
    });
    // {{ param | go_param_type }} → Go arg type for a parameter (pointer if optional)
    tera.register_filter("go_param_type", |v: &Value, _: &HashMap<String, Value>| {
        let required = v.get("required").and_then(Value::as_bool).unwrap_or(false);
        let type_ref = v.get("type_ref").cloned().unwrap_or(Value::Null);
        let t = if required { go_required_type_from_value(&type_ref) } else { go_optional_type_from_value(&type_ref) };
        Ok(Value::String(t))
    });
    // {{ request_body | go_body_type }} → Go body arg type
    tera.register_filter("go_body_type", |v: &Value, _: &HashMap<String, Value>| {
        let required = v.get("required").and_then(Value::as_bool).unwrap_or(false);
        Ok(Value::String(go_body_type_from_value(v, required)))
    });
    // {{ operation | go_return_shape }} → {has_result, signature, result_go_type, decode_go_type, returns_nil_on_error, returns_pointer, content_encoding}
    tera.register_filter("go_return_shape", |v: &Value, _: &HashMap<String, Value>| {
        Ok(build_return_shape(v))
    });
    // {{ operation | go_args_sig }} → Go function argument list string
    tera.register_filter("go_args_sig", |v: &Value, _: &HashMap<String, Value>| {
        Ok(Value::String(build_args_sig(v)))
    });
    // {{ operation | go_forward_args }} → Go forwarding argument list string
    tera.register_filter("go_forward_args", |v: &Value, _: &HashMap<String, Value>| {
        Ok(Value::String(build_forward_args(v)))
    });
    // {{ operation | go_body_view }} → {required, kind, content_encoding} or null
    tera.register_filter("go_body_view", |v: &Value, _: &HashMap<String, Value>| {
        Ok(build_body_view(v))
    });
    // {{ "get" | go_method }} → "http.MethodGet"
    tera.register_filter("go_method", |v: &Value, _: &HashMap<String, Value>| {
        let raw = v.as_str().unwrap_or("");
        let constant = match raw.to_uppercase().as_str() {
            "GET"    => "http.MethodGet".into(),
            "POST"   => "http.MethodPost".into(),
            "PUT"    => "http.MethodPut".into(),
            "PATCH"  => "http.MethodPatch".into(),
            "DELETE" => "http.MethodDelete".into(),
            other    => format!("http.Method{}", capitalize(other)),
        };
        Ok(Value::String(constant))
    });
    // {{ "/users/{userId}" | go_path_format }} → "/users/%s"
    tera.register_filter("go_path_format", |v: &Value, _: &HashMap<String, Value>| {
        Ok(Value::String(build_path_format(v.as_str().unwrap_or(""))))
    });
    // {{ operations | go_tag_groups }} → [{tag, field_name, struct_name, operations}]
    tera.register_filter("go_tag_groups", |v: &Value, _: &HashMap<String, Value>| {
        Ok(build_tag_groups(v))
    });
    // {{ operation | go_primary_tag }} → primary tag string or ""
    tera.register_filter("go_primary_tag", |v: &Value, _: &HashMap<String, Value>| {
        let tag = v.get("attributes")
            .and_then(|a| a.get("tags"))
            .and_then(Value::as_array)
            .and_then(|tags| tags.first())
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("");
        Ok(Value::String(tag.into()))
    });
    // {{ "text\nwith newlines" | go_comment }} → "text  with newlines"
    tera.register_filter("go_comment", |v: &Value, _: &HashMap<String, Value>| {
        Ok(Value::String(v.as_str().unwrap_or("").replace('\n', " ").replace('\r', " ")))
    });
}

// ── Type helpers ──────────────────────────────────────────────────────────────

fn go_type_from_value(v: &Value) -> String {
    match v.get("kind").and_then(Value::as_str) {
        Some("primitive") => match v["name"].as_str().unwrap_or("") {
            "string"  => "string",
            "integer" => "int64",
            "number"  => "float64",
            "boolean" => "bool",
            "binary"  => "[]byte",
            _         => "any",
        }.into(),
        Some("named") => sanitize_exported_identifier(v["name"].as_str().unwrap_or("")),
        Some("array") => format!("[]{}", go_type_from_value(&v["item"])),
        Some("map")   => format!("map[string]{}", go_type_from_value(&v["value"])),
        _             => "any".into(),
    }
}

fn go_field_type_from_value(type_ref: &Value, optional: bool, nullable: bool) -> String {
    if optional || nullable {
        match type_ref.get("kind").and_then(Value::as_str) {
            Some("primitive") => match type_ref["name"].as_str().unwrap_or("") {
                "string"  => return "*string".into(),
                "integer" => return "*int64".into(),
                "number"  => return "*float64".into(),
                "boolean" => return "*bool".into(),
                _ => {}
            },
            Some("named") => return format!("*{}", go_type_from_value(type_ref)),
            _ => {}
        }
    }
    go_type_from_value(type_ref)
}

fn go_required_type_from_value(type_ref: &Value) -> String {
    go_type_from_value(type_ref)
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None    => String::new(),
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
    }
}

fn go_optional_type_from_value(type_ref: &Value) -> String {
    match type_ref.get("kind").and_then(Value::as_str) {
        Some("primitive") => match type_ref["name"].as_str().unwrap_or("") {
            "string"  => "*string".into(),
            "integer" => "*int64".into(),
            "number"  => "*float64".into(),
            "boolean" => "*bool".into(),
            _         => go_type_from_value(type_ref),
        },
        Some("named") => format!("*{}", go_type_from_value(type_ref)),
        _             => go_type_from_value(type_ref),
    }
}

fn go_body_type_from_value(body: &Value, required: bool) -> String {
    let type_ref = body.get("type_ref").filter(|v| !v.is_null());
    match type_ref {
        Some(v) if v.get("kind").and_then(Value::as_str) == Some("named") => {
            format!("*{}", sanitize_exported_identifier(v["name"].as_str().unwrap_or("")))
        }
        Some(v) => {
            if required { go_required_type_from_value(v) } else { go_optional_type_from_value(v) }
        }
        None => "io.Reader".into(),
    }
}

// ── Operation helpers ─────────────────────────────────────────────────────────

fn build_path_format(path: &str) -> String {
    let mut result = String::new();
    let mut chars = path.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '{' => {
                while chars.peek().is_some_and(|&c| c != '}') { chars.next(); }
                chars.next(); // consume '}'
                result.push_str("%s");
            }
            '%' => result.push_str("%%"),
            _   => result.push(ch),
        }
    }
    result
}

fn build_args_sig(op: &Value) -> String {
    let mut args = vec!["ctx context.Context".into()];
    let params = op.get("params").and_then(Value::as_array);
    // Required params
    if let Some(ps) = params {
        for p in ps.iter().filter(|p| p.get("required").and_then(Value::as_bool).unwrap_or(false)) {
            let name = sanitize_identifier(p.get("name").and_then(Value::as_str).unwrap_or(""));
            let t = go_required_type_from_value(p.get("type_ref").unwrap_or(&Value::Null));
            args.push(format!("{name} {t}"));
        }
    }
    // Required body
    if let Some(body) = op.get("request_body").filter(|v| !v.is_null()) {
        if body.get("required").and_then(Value::as_bool).unwrap_or(false) {
            args.push(format!("body {}", go_body_type_from_value(body, true)));
        }
    }
    // Optional params
    if let Some(ps) = params {
        for p in ps.iter().filter(|p| !p.get("required").and_then(Value::as_bool).unwrap_or(false)) {
            let name = sanitize_identifier(p.get("name").and_then(Value::as_str).unwrap_or(""));
            let t = go_optional_type_from_value(p.get("type_ref").unwrap_or(&Value::Null));
            args.push(format!("{name} {t}"));
        }
    }
    // Optional body
    if let Some(body) = op.get("request_body").filter(|v| !v.is_null()) {
        if !body.get("required").and_then(Value::as_bool).unwrap_or(false) {
            args.push(format!("body {}", go_body_type_from_value(body, false)));
        }
    }
    args.push("requestOptions *RequestOptions".into());
    args.join(", ")
}

fn build_forward_args(op: &Value) -> String {
    let mut args = vec!["ctx".into()];
    let params = op.get("params").and_then(Value::as_array);
    if let Some(ps) = params {
        for p in ps.iter().filter(|p| p.get("required").and_then(Value::as_bool).unwrap_or(false)) {
            args.push(sanitize_identifier(p.get("name").and_then(Value::as_str).unwrap_or("")));
        }
    }
    if let Some(body) = op.get("request_body").filter(|v| !v.is_null()) {
        if body.get("required").and_then(Value::as_bool).unwrap_or(false) {
            args.push("body".into());
        }
    }
    if let Some(ps) = params {
        for p in ps.iter().filter(|p| !p.get("required").and_then(Value::as_bool).unwrap_or(false)) {
            args.push(sanitize_identifier(p.get("name").and_then(Value::as_str).unwrap_or("")));
        }
    }
    if let Some(body) = op.get("request_body").filter(|v| !v.is_null()) {
        if !body.get("required").and_then(Value::as_bool).unwrap_or(false) {
            args.push("body".into());
        }
    }
    args.push("requestOptions".into());
    args.join(", ")
}

fn build_return_shape(op: &Value) -> Value {
    let success = op.get("responses")
        .and_then(Value::as_array)
        .and_then(|rs| rs.iter().find(|r| r.get("status").and_then(Value::as_str).is_some_and(|s| s.starts_with('2'))));
    let content_encoding = success
        .and_then(|r| r.get("attributes"))
        .and_then(|a| a.get("content_encoding"))
        .cloned()
        .unwrap_or(Value::Null);
    let type_ref = success
        .and_then(|r| r.get("type_ref"))
        .filter(|v| !v.is_null())
        .cloned();
    match type_ref {
        Some(ref tr) => {
            let result_t = go_result_type_from_value(tr);
            let decode_t = go_decode_type_from_value(tr);
            json!({
                "has_result": true,
                "signature": format!("({result_t}, error)"),
                "result_go_type": result_t,
                "decode_go_type": decode_t,
                "returns_nil_on_error": returns_nil_from_value(tr),
                "returns_pointer": returns_pointer_from_value(tr),
                "content_encoding": content_encoding,
            })
        }
        None => json!({
            "has_result": false,
            "signature": "error",
            "result_go_type": "",
            "decode_go_type": "",
            "returns_nil_on_error": false,
            "returns_pointer": false,
            "content_encoding": content_encoding,
        }),
    }
}

fn go_result_type_from_value(tr: &Value) -> String {
    if returns_pointer_from_value(tr) { format!("*{}", go_decode_type_from_value(tr)) } else { go_decode_type_from_value(tr) }
}

fn go_decode_type_from_value(tr: &Value) -> String {
    match tr.get("kind").and_then(Value::as_str) {
        Some("named") => sanitize_exported_identifier(tr["name"].as_str().unwrap_or("")),
        _             => go_type_from_value(tr),
    }
}

fn returns_pointer_from_value(tr: &Value) -> bool {
    tr.get("kind").and_then(Value::as_str) == Some("named")
}

fn returns_nil_from_value(tr: &Value) -> bool {
    match tr.get("kind").and_then(Value::as_str) {
        Some("named") | Some("array") | Some("map") => true,
        Some("primitive") => tr["name"].as_str() == Some("binary"),
        _ => false,
    }
}

fn build_body_view(op: &Value) -> Value {
    let body = match op.get("request_body").filter(|v| !v.is_null()) {
        Some(b) => b,
        None    => return Value::Null,
    };
    let required = body.get("required").and_then(Value::as_bool).unwrap_or(false);
    let media_type = body.get("media_type").and_then(Value::as_str).unwrap_or("");
    let kind = if media_type == "application/json" {
        "json"
    } else if media_type.starts_with("multipart/form-data") {
        "multipart"
    } else {
        "binary"
    };
    let content_encoding = body.get("attributes")
        .and_then(|a| a.get("content_encoding"))
        .cloned()
        .unwrap_or(Value::Null);
    json!({ "required": required, "kind": kind, "content_encoding": content_encoding })
}

fn build_tag_groups(ops: &Value) -> Value {
    let operations = match ops.as_array() {
        Some(ops) => ops,
        None      => return Value::Array(vec![]),
    };
    let mut groups: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for op in operations {
        if let Some(tag) = op.get("attributes")
            .and_then(|a| a.get("tags"))
            .and_then(Value::as_array)
            .and_then(|tags| tags.first())
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|t| !t.is_empty())
        {
            groups.entry(tag.to_owned()).or_default().push(op.clone());
        }
    }
    Value::Array(
        groups.into_iter().map(|(tag, ops)| {
            let field_name = sanitize_exported_identifier(&tag);
            let struct_name = format!("{field_name}Service");
            json!({ "tag": tag, "field_name": field_name, "struct_name": struct_name, "operations": ops })
        }).collect(),
    )
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Generate a Go client package from an IR snapshot.
///
/// Convenience alias for the [`generate`] function produced by [`declare_target!`].
pub fn generate_go_package(
    ir: &CoreIr,
    template_dir: Option<&std::path::Path>,
    common: &CommonConfig,
    config: &TargetConfig,
) -> Result<Vec<GeneratedFile>> {
    generate(ir, template_dir, common, config)
}

pub use arvalez_target_core::write_files as write_go_package;

arvalez_target_core::declare_target! {
    config:    TargetConfig,
    templates: TEMPLATES,
    filters:   register_filters,
}
