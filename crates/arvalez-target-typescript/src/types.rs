use arvalez_ir::{HttpMethod, Operation, TypeRef};
use serde_json::Value;

use crate::sanitize::{sanitize_type_name, sanitize_identifier};

pub(crate) fn typescript_field_type(type_ref: &TypeRef, nullable: bool) -> String {
    let mut ty = typescript_type_ref(type_ref);
    if nullable {
        ty.push_str(" | null");
    }
    ty
}

pub(crate) fn typescript_type_ref(type_ref: &TypeRef) -> String {
    match type_ref {
        TypeRef::Primitive { name } => match name.as_str() {
            "string" => "string".into(),
            "integer" | "number" => "number".into(),
            "boolean" => "boolean".into(),
            "binary" => "Blob".into(),
            "null" => "null".into(),
            "any" | "object" => "JsonValue".into(),
            _ => "unknown".into(),
        },
        TypeRef::Named { name } => sanitize_type_name(name),
        TypeRef::Array { item } => format!("{}[]", typescript_type_ref(item)),
        TypeRef::Map { value } => format!("Record<string, {}>", typescript_type_ref(value)),
        TypeRef::Union { variants } => variants
            .iter()
            .map(typescript_type_ref)
            .collect::<Vec<_>>()
            .join(" | "),
    }
}

pub(crate) fn render_typescript_enum_variant(value: &Value) -> String {
    match value {
        Value::String(value) => format!("{value:?}"),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::Null => "null".into(),
        _ => "unknown".into(),
    }
}

pub(crate) fn http_method_string(method: HttpMethod) -> &'static str {
    match method {
        HttpMethod::Get => "GET",
        HttpMethod::Post => "POST",
        HttpMethod::Put => "PUT",
        HttpMethod::Patch => "PATCH",
        HttpMethod::Delete => "DELETE",
    }
}

pub(crate) fn render_typescript_path(path: &str) -> String {
    let mut result = String::from("`");
    let mut chars = path.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '{' => {
                let mut name = String::new();
                while let Some(next) = chars.peek() {
                    if *next == '}' {
                        chars.next();
                        break;
                    }
                    name.push(*next);
                    chars.next();
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

pub(crate) fn operation_return_type(operation: &Operation) -> String {
    operation
        .responses
        .iter()
        .find(|response| response.status.starts_with('2'))
        .and_then(|response| response.type_ref.as_ref())
        .map(typescript_type_ref)
        .unwrap_or_else(|| "void".into())
}

pub(crate) fn raw_method_name(operation: &Operation) -> String {
    format!("_{}Raw", sanitize_identifier(&operation.name))
}

pub(crate) fn build_wrapper_forward_arguments(operation: &Operation) -> String {
    let mut args = Vec::new();
    for param in operation.params.iter().filter(|param| param.required) {
        args.push(sanitize_identifier(&param.name));
    }
    if let Some(request_body) = &operation.request_body
        && request_body.required
    {
        let _ = request_body;
        args.push("body".into());
    }
    for param in operation.params.iter().filter(|param| !param.required) {
        args.push(sanitize_identifier(&param.name));
    }
    if let Some(request_body) = &operation.request_body
        && !request_body.required
    {
        let _ = request_body;
        args.push("body".into());
    }
    args.push("requestOptions".into());
    args.join(", ")
}

pub(crate) fn build_method_args(operation: &Operation) -> Vec<String> {
    let mut args = Vec::new();
    for param in operation.params.iter().filter(|param| param.required) {
        args.push(format!(
            "{}: {}",
            sanitize_identifier(&param.name),
            typescript_type_ref(&param.type_ref)
        ));
    }
    if let Some(request_body) = &operation.request_body
        && request_body.required
    {
        args.push(format!(
            "body: {}",
            request_body
                .type_ref
                .as_ref()
                .map(typescript_type_ref)
                .unwrap_or_else(|| "unknown".into())
        ));
    }
    for param in operation.params.iter().filter(|param| !param.required) {
        args.push(format!(
            "{}?: {}",
            sanitize_identifier(&param.name),
            typescript_type_ref(&param.type_ref)
        ));
    }
    if let Some(request_body) = &operation.request_body
        && !request_body.required
    {
        args.push(format!(
            "body?: {}",
            request_body
                .type_ref
                .as_ref()
                .map(typescript_type_ref)
                .unwrap_or_else(|| "unknown".into())
        ));
    }
    args.push("requestOptions?: RequestOptions".into());
    args
}
