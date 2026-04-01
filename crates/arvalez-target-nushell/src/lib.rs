//! Minimal Nushell SDK generator.
//!
//! The only Rust logic here is Nushell-specific string sanitisation and
//! TypeRef → Nushell type conversion.  All code-structure decisions live in
//! the Tera templates.  [`declare_target!`] wires the static parts together.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tera::Tera;

// ── Config ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetConfig {
    /// Default base URL embedded in generated command stubs.
    pub default_base_url: String,
    /// When `true`, commands are emitted as `"tag-name command-name"` subcommands.
    #[serde(default)]
    pub group_by_tag: bool,
}

impl Default for TargetConfig {
    fn default() -> Self {
        Self { default_base_url: String::new(), group_by_tag: false }
    }
}

// ── Templates ─────────────────────────────────────────────────────────────────

/// Template name encodes the output path: strip the `root/` prefix, strip `.tera`,
/// then expand `{var}` placeholders.  Templates under `partials/` are included by
/// others and are never rendered directly to a file.
pub const TEMPLATES: &[(&str, &str)] = &[
    ("root/README.md.tera", include_str!("../templates/root/README.md.tera")),
    ("root/mod.nu.tera", include_str!("../templates/root/mod.nu.tera")),
    ("root/client.nu.tera", include_str!("../templates/root/client.nu.tera")),
    ("root/models.nu.tera", include_str!("../templates/root/models.nu.tera")),
    (
        "partials/command.nu.tera",
        include_str!("../templates/partials/command.nu.tera"),
    ),
    (
        "partials/model_record.nu.tera",
        include_str!("../templates/partials/model_record.nu.tera"),
    ),
];

// ── Tera filters ──────────────────────────────────────────────────────────────

pub fn register_filters(tera: &mut Tera) {
    // {{ type_ref | nu_type }} — TypeRef JSON → Nushell type annotation
    tera.register_filter("nu_type", |v: &Value, _: &HashMap<String, Value>| {
        Ok(Value::String(type_ref_to_nu(v)))
    });
    // {{ "someIdentifier" | nu_var }} — string → snake_case Nushell variable name (keyword-escaped)
    tera.register_filter("nu_var", |v: &Value, _: &HashMap<String, Value>| {
        Ok(Value::String(sanitize_variable(v.as_str().unwrap_or(""))))
    });
    // {{ "someIdentifier" | nu_cmd }} — string → kebab-case Nushell command / flag name
    tera.register_filter("nu_cmd", |v: &Value, _: &HashMap<String, Value>| {
        Ok(Value::String(sanitize_command(v.as_str().unwrap_or(""))))
    });
    // {{ "/users/{userId}" | nu_path }} → "/users/($user_id)"
    tera.register_filter("nu_path", |v: &Value, _: &HashMap<String, Value>| {
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
                out.push('(');
                out.push('$');
                out.push_str(&sanitize_variable(&name));
                out.push(')');
            } else {
                out.push(ch);
            }
        }
        Ok(Value::String(out))
    });
    // {{ model.fields | nu_typed_record }} → "record<id: string, count: int>"
    tera.register_filter("nu_typed_record", |v: &Value, _: &HashMap<String, Value>| {
        let fields = match v.as_array() {
            Some(f) => f,
            None => return Ok(Value::String("record".into())),
        };
        if fields.is_empty() {
            return Ok(Value::String("record".into()));
        }
        let parts: Vec<String> = fields
            .iter()
            .map(|f| {
                let name = f.get("name").and_then(Value::as_str).unwrap_or("field");
                let type_val = f.get("type_ref").unwrap_or(&Value::Null);
                format!("{}: {}", sanitize_variable(name), type_ref_to_nu(type_val))
            })
            .collect();
        Ok(Value::String(format!("record<{}>", parts.join(", "))))
    });
}

// ── TypeRef → Nushell type ────────────────────────────────────────────────────

fn type_ref_to_nu(v: &Value) -> String {
    match v.get("kind").and_then(Value::as_str) {
        Some("primitive") => match v["name"].as_str().unwrap_or("") {
            "string" => "string",
            "integer" => "int",
            "number" => "float",
            "boolean" => "bool",
            "binary" => "binary",
            _ => "any",
        }
        .into(),
        Some("named") => "record".into(),
        Some("array") => format!("list<{}>", type_ref_to_nu(&v["item"])),
        Some("map") => "record".into(),
        Some("union") => "any".into(),
        _ => "any".into(),
    }
}

// ── Identifier helpers ────────────────────────────────────────────────────────

fn split_words(name: &str) -> Vec<String> {
    let mut words: Vec<String> = Vec::new();
    let mut current = String::new();
    for ch in name.chars() {
        if ch == '_' || ch == '-' || ch == ' ' || ch == '.' {
            if !current.is_empty() {
                words.push(current.to_lowercase());
                current.clear();
            }
        } else if ch.is_uppercase() && !current.is_empty() {
            words.push(current.to_lowercase());
            current.clear();
            current.push(ch);
        } else {
            current.push(ch);
        }
    }
    if !current.is_empty() {
        words.push(current.to_lowercase());
    }
    words
}

fn sanitize_variable(name: &str) -> String {
    let words = split_words(name);
    let candidate = if words.is_empty() { "value".into() } else { words.join("_") };
    if candidate.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        format!("_{candidate}")
    } else if is_nushell_keyword(&candidate) {
        format!("{candidate}_")
    } else {
        candidate
    }
}

fn sanitize_command(name: &str) -> String {
    let words = split_words(name);
    if words.is_empty() { "command".into() } else { words.join("-") }
}

fn is_nushell_keyword(name: &str) -> bool {
    matches!(
        name,
        "let" | "mut" | "if" | "else" | "for" | "in" | "while" | "loop"
            | "match" | "def" | "export" | "use" | "module" | "return"
            | "true" | "false" | "null" | "do" | "try" | "catch" | "from"
            | "into" | "not" | "and" | "or" | "xor" | "bit-and" | "bit-or"
    )
}

arvalez_target_core::declare_target! {
    config:    TargetConfig,
    templates: TEMPLATES,
    filters:   register_filters,
}

#[cfg(test)]
mod tests {
    use arvalez_ir::{
        Attributes, CoreIr, Field, HttpMethod, Operation, Parameter, ParameterLocation,
        RequestBody, Response, TypeRef,
    };
    use arvalez_target_core::CommonConfig;
    use serde_json::{Value, json};

    use crate::{TargetConfig, generate};

    fn sample_ir() -> CoreIr {
        CoreIr {
            models: vec![arvalez_ir::Model {
                id: "model.widget".into(),
                name: "Widget".into(),
                fields: vec![
                    Field::new("id", TypeRef::primitive("string")),
                    Field {
                        name: "count".into(),
                        type_ref: TypeRef::primitive("integer"),
                        optional: true,
                        nullable: false,
                        attributes: Attributes::default(),
                    },
                ],
                attributes: Attributes::default(),
                source: None,
            }],
            operations: vec![Operation {
                id: "operation.get_widget".into(),
                name: "get_widget".into(),
                method: HttpMethod::Get,
                path: "/widgets/{widget_id}".into(),
                params: vec![
                    Parameter {
                        name: "widget_id".into(),
                        location: ParameterLocation::Path,
                        type_ref: TypeRef::primitive("string"),
                        required: true,
                        attributes: Attributes::from([(
                            "description".into(),
                            Value::String("Unique widget identifier.".into()),
                        )]),
                    },
                    Parameter {
                        name: "include_count".into(),
                        location: ParameterLocation::Query,
                        type_ref: TypeRef::primitive("boolean"),
                        required: false,
                        attributes: Attributes::default(),
                    },
                ],
                request_body: None,
                responses: vec![Response {
                    status: "200".into(),
                    media_type: Some("application/json".into()),
                    type_ref: Some(TypeRef::named("Widget")),
                    attributes: Attributes::default(),
                }],
                attributes: Attributes::from([("tags".into(), json!(["widgets"]))]),
                source: None,
            }],
            ..Default::default()
        }
    }

    fn post_ir() -> CoreIr {
        CoreIr {
            models: vec![],
            operations: vec![Operation {
                id: "operation.create_widget".into(),
                name: "create_widget".into(),
                method: HttpMethod::Post,
                path: "/widgets".into(),
                params: vec![],
                request_body: Some(RequestBody {
                    required: true,
                    media_type: "application/json".into(),
                    type_ref: None,
                    attributes: Attributes::default(),
                }),
                responses: vec![Response {
                    status: "201".into(),
                    media_type: Some("application/json".into()),
                    type_ref: None,
                    attributes: Attributes::default(),
                }],
                attributes: Attributes::default(),
                source: None,
            }],
            ..Default::default()
        }
    }

    fn default_common() -> CommonConfig {
        CommonConfig { package_name: "my-api".into(), version: "0.1.0".into() }
    }

    #[test]
    fn renders_basic_nushell_package() {
        let common =
            CommonConfig { package_name: "my-api".into(), version: "1.0.0".into() };
        let config = TargetConfig {
            default_base_url: "https://api.example.com".into(),
            group_by_tag: false,
        };

        let files = generate(&sample_ir(), None, &common, &config).expect("should render");

        let paths: Vec<_> = files.iter().map(|f| f.path.to_str().unwrap()).collect();
        assert!(paths.contains(&"README.md"), "README.md missing");
        assert!(paths.contains(&"mod.nu"), "mod.nu missing");
        assert!(paths.contains(&"client.nu"), "client.nu missing");
        assert!(paths.contains(&"models.nu"), "models.nu missing");
    }

    #[test]
    fn client_contains_command_and_path_param() {
        let config = TargetConfig::default();
        let files =
            generate(&sample_ir(), None, &default_common(), &config).expect("should render");

        let client = files.iter().find(|f| f.path.ends_with("client.nu")).expect("client.nu");

        assert!(client.contents.contains("get-widget"), "command name not found");
        assert!(client.contents.contains("widget_id"), "path param not found");
        assert!(client.contents.contains("http get"), "http verb not found");
    }

    #[test]
    fn models_contains_make_command() {
        let config = TargetConfig::default();
        let files =
            generate(&sample_ir(), None, &default_common(), &config).expect("should render");

        let models = files.iter().find(|f| f.path.ends_with("models.nu")).expect("models.nu");

        assert!(
            models.contents.contains("make-widget"),
            "model constructor not found: {}",
            models.contents
        );
    }

    #[test]
    fn post_command_includes_body() {
        let config = TargetConfig::default();
        let files =
            generate(&post_ir(), None, &default_common(), &config).expect("should render");

        let client = files.iter().find(|f| f.path.ends_with("client.nu")).expect("client.nu");

        assert!(client.contents.contains("create-widget"), "command not found");
        assert!(
            client.contents.contains("--body"),
            "--body flag not found in: {}",
            client.contents
        );
        assert!(client.contents.contains("http post"), "http post not found");
    }

    #[test]
    fn models_use_typed_record_return() {
        let config = TargetConfig::default();
        let files =
            generate(&sample_ir(), None, &default_common(), &config).expect("should render");

        let models = files.iter().find(|f| f.path.ends_with("models.nu")).expect("models.nu");

        // The Widget model has id: string and count: int, so the constructor should
        // return a typed record rather than the bare `record` annotation.
        assert!(
            models.contents.contains("record<"),
            "typed record annotation not found in: {}",
            models.contents
        );
        assert!(
            models.contents.contains("id: string"),
            "typed field id: string not found in: {}",
            models.contents
        );
    }

    #[test]
    fn group_by_tag_prefixes_command_name() {
        let config = TargetConfig { default_base_url: String::new(), group_by_tag: true };
        let files =
            generate(&sample_ir(), None, &default_common(), &config).expect("should render");

        let client = files.iter().find(|f| f.path.ends_with("client.nu")).expect("client.nu");

        // The get_widget operation has tag "widgets", so with group_by_tag the
        // export should be `"widgets get-widget"`.
        assert!(
            client.contents.contains("widgets get-widget"),
            "tagged subcommand not found in: {}",
            client.contents
        );
    }

    #[test]
    fn no_group_by_tag_uses_flat_command_name() {
        let config = TargetConfig::default();
        let files =
            generate(&sample_ir(), None, &default_common(), &config).expect("should render");

        let client = files.iter().find(|f| f.path.ends_with("client.nu")).expect("client.nu");

        // Without group_by_tag the command should just be `"get-widget"`.
        assert!(
            client.contents.contains("\"get-widget\""),
            "flat command name not found in: {}",
            client.contents
        );
        assert!(
            !client.contents.contains("\"widgets get-widget\""),
            "tag prefix should not appear without group_by_tag"
        );
    }
}
