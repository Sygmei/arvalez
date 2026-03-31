use arvalez_ir::{HttpMethod, Operation, Response, TypeRef};
use serde_json::Value;

use crate::sanitize::{sanitize_class_name, sanitize_identifier};

#[derive(Debug, Clone, Copy)]
pub(crate) enum PythonContext {
    Models,
    Client,
}

#[derive(Debug, Clone)]
pub(crate) struct ReturnType {
    pub(crate) annotation: Option<String>,
    pub(crate) parse_expression: Option<String>,
}

pub(crate) fn python_field_type(type_ref: &TypeRef, optional: bool, nullable: bool) -> String {
    let mut type_hint = python_type_ref(type_ref, PythonContext::Models);
    if optional || nullable {
        type_hint.push_str(" | None");
    }
    type_hint
}

pub(crate) fn python_type_ref(type_ref: &TypeRef, context: PythonContext) -> String {
    match type_ref {
        TypeRef::Primitive { name } => match name.as_str() {
            "string" => "str".into(),
            "integer" => "int".into(),
            "number" => "float".into(),
            "boolean" => "bool".into(),
            "binary" => "bytes".into(),
            "null" => "None".into(),
            "any" | "object" => "Any".into(),
            _ => "Any".into(),
        },
        TypeRef::Named { name } => match context {
            PythonContext::Models => sanitize_class_name(name),
            PythonContext::Client => format!("models.{}", sanitize_class_name(name)),
        },
        TypeRef::Array { item } => format!("list[{}]", python_type_ref(item, context)),
        TypeRef::Map { value } => format!("dict[str, {}]", python_type_ref(value, context)),
        TypeRef::Union { variants } => variants
            .iter()
            .map(|variant| python_type_ref(variant, context))
            .collect::<Vec<_>>()
            .join(" | "),
    }
}

pub(crate) fn method_literal(method: HttpMethod) -> &'static str {
    match method {
        HttpMethod::Get => "GET",
        HttpMethod::Post => "POST",
        HttpMethod::Put => "PUT",
        HttpMethod::Patch => "PATCH",
        HttpMethod::Delete => "DELETE",
    }
}

pub(crate) fn operation_return_type(operation: &Operation) -> ReturnType {
    let success = operation
        .responses
        .iter()
        .find(|response| response.status.starts_with('2'));
    success.map(response_return_type).unwrap_or(ReturnType {
        annotation: None,
        parse_expression: None,
    })
}

pub(crate) fn response_return_type(response: &Response) -> ReturnType {
    match &response.type_ref {
        Some(type_ref) => {
            let type_hint = python_type_ref(type_ref, PythonContext::Client);
            ReturnType {
                annotation: Some(type_hint.clone()),
                parse_expression: Some(type_hint),
            }
        }
        None => ReturnType {
            annotation: Some("None".into()),
            parse_expression: None,
        },
    }
}

pub(crate) fn enum_base_classes(model: &arvalez_ir::Model) -> String {
    match model
        .attributes
        .get("enum_base_type")
        .and_then(|value| value.as_str())
    {
        Some("string") => "str, Enum".into(),
        Some("integer") => "int, Enum".into(),
        Some("number") => "float, Enum".into(),
        _ => "Enum".into(),
    }
}

pub(crate) fn render_enum_member(value: &Value, index: usize) -> String {
    use crate::sanitize::sanitize_enum_member_name;
    let member_name = value
        .as_str()
        .map(sanitize_enum_member_name)
        .unwrap_or_else(|| format!("VALUE_{index}"));
    format!("{member_name} = {}", python_literal(value))
}

pub(crate) fn python_literal(value: &Value) -> String {
    match value {
        Value::String(value) => format!("{value:?}"),
        Value::Number(value) => value.to_string(),
        Value::Bool(value) => {
            if *value {
                "True".into()
            } else {
                "False".into()
            }
        }
        Value::Null => "None".into(),
        _ => serde_json::to_string(value).unwrap_or_else(|_| "None".into()),
    }
}
