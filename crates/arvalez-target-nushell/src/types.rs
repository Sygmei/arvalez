use std::collections::HashMap;

use arvalez_ir::{CoreIr, HttpMethod, TypeRef};
use arvalez_target_core::sorted_models;

use crate::sanitize::{sanitize_field_name, sanitize_variable_name};

/// Maps IR model names to their Nushell typed record annotations.
/// Enums resolve to `"string"`; object types to `"record<field: type, ...>"`.
pub(crate) type TypeRegistry = HashMap<String, String>;

/// Build a [`TypeRegistry`] from the IR models.
pub(crate) fn build_type_registry(ir: &CoreIr) -> TypeRegistry {
    let mut map = HashMap::new();
    for model in sorted_models(ir) {
        if model.attributes.contains_key("enum_values") {
            map.insert(model.name.clone(), "string".to_owned());
        } else {
            let fields: Vec<String> = model
                .fields
                .iter()
                .map(|f| format!("{}: {}", sanitize_field_name(&f.name), plain_type_ref(&f.type_ref)))
                .collect();
            let record_type = if fields.is_empty() {
                "record".to_owned()
            } else {
                format!("record<{}>", fields.join(", "))
            };
            map.insert(model.name.clone(), record_type);
        }
    }
    map
}

/// Map an IR [`TypeRef`] to a Nushell type annotation, resolving named types
/// through the registry.
pub(crate) fn nushell_type_ref(type_ref: &TypeRef, registry: &TypeRegistry) -> String {
    match type_ref {
        TypeRef::Primitive { name } => primitive_name(name),
        TypeRef::Named { name } => registry.get(name).cloned().unwrap_or_else(|| "record".into()),
        TypeRef::Array { item } => format!("list<{}>", nushell_type_ref(item, registry)),
        TypeRef::Map { .. } => "record".into(),
        TypeRef::Union { .. } => "any".into(),
    }
}

/// Resolve a type without registry lookup — used during registry construction
/// to avoid circular references.
fn plain_type_ref(type_ref: &TypeRef) -> String {
    match type_ref {
        TypeRef::Primitive { name } => primitive_name(name),
        TypeRef::Named { .. } => "record".into(),
        TypeRef::Array { item } => format!("list<{}>", plain_type_ref(item)),
        TypeRef::Map { .. } => "record".into(),
        TypeRef::Union { .. } => "any".into(),
    }
}

fn primitive_name(name: &str) -> String {
    match name {
        "string" => "string".into(),
        "integer" => "int".into(),
        "number" => "float".into(),
        "boolean" => "bool".into(),
        "binary" => "binary".into(),
        _ => "any".into(),
    }
}

/// Return the Nushell HTTP verb keyword for a method.
pub(crate) fn http_verb(method: HttpMethod) -> &'static str {
    match method {
        HttpMethod::Get => "get",
        HttpMethod::Post => "post",
        HttpMethod::Put => "put",
        HttpMethod::Patch => "patch",
        HttpMethod::Delete => "delete",
    }
}

/// Render a Nushell path template where `{name}` becomes `($var)`.
pub(crate) fn render_nu_path(path: &str) -> String {
    let mut result = String::new();
    let mut chars = path.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '{' {
            let mut name = String::new();
            for next in chars.by_ref() {
                if next == '}' {
                    break;
                }
                name.push(next);
            }
            result.push('(');
            result.push('$');
            result.push_str(&sanitize_variable_name(&name));
            result.push(')');
        } else {
            result.push(ch);
        }
    }
    result
}
