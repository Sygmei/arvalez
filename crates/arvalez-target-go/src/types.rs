use arvalez_ir::{HttpMethod, RequestBody, TypeRef};

use crate::sanitize::sanitize_exported_identifier;

pub(crate) fn go_field_type(type_ref: &TypeRef, optional: bool, nullable: bool) -> String {
    let base = go_type_ref(type_ref);
    if optional || nullable {
        match type_ref {
            TypeRef::Primitive { name } if name == "string" => "*string".into(),
            TypeRef::Primitive { name } if name == "integer" => "*int64".into(),
            TypeRef::Primitive { name } if name == "number" => "*float64".into(),
            TypeRef::Primitive { name } if name == "boolean" => "*bool".into(),
            TypeRef::Named { .. } => format!("*{base}"),
            _ => base,
        }
    } else {
        base
    }
}

pub(crate) fn go_type_ref(type_ref: &TypeRef) -> String {
    match type_ref {
        TypeRef::Primitive { name } => match name.as_str() {
            "string" => "string".into(),
            "integer" => "int64".into(),
            "number" => "float64".into(),
            "boolean" => "bool".into(),
            "binary" => "[]byte".into(),
            "null" => "any".into(),
            "any" | "object" => "any".into(),
            _ => "any".into(),
        },
        TypeRef::Named { name } => sanitize_exported_identifier(name),
        TypeRef::Array { item } => format!("[]{}", go_type_ref(item)),
        TypeRef::Map { value } => format!("map[string]{}", go_type_ref(value)),
        TypeRef::Union { .. } => "any".into(),
    }
}

pub(crate) fn go_body_arg_type(request_body: &RequestBody, required: bool) -> String {
    match request_body.type_ref.as_ref() {
        Some(TypeRef::Named { name }) => format!("*{}", sanitize_exported_identifier(name)),
        Some(type_ref) => {
            let base = go_type_ref(type_ref);
            if required {
                base
            } else {
                match type_ref {
                    TypeRef::Primitive { name } if name == "string" => "*string".into(),
                    TypeRef::Primitive { name } if name == "integer" => "*int64".into(),
                    TypeRef::Primitive { name } if name == "number" => "*float64".into(),
                    TypeRef::Primitive { name } if name == "boolean" => "*bool".into(),
                    _ => base,
                }
            }
        }
        None => "io.Reader".into(),
    }
}

pub(crate) fn go_required_arg_type(type_ref: &TypeRef) -> String {
    go_type_ref(type_ref)
}

pub(crate) fn go_optional_arg_type(type_ref: &TypeRef) -> String {
    match type_ref {
        TypeRef::Primitive { name } if name == "string" => "*string".into(),
        TypeRef::Primitive { name } if name == "integer" => "*int64".into(),
        TypeRef::Primitive { name } if name == "number" => "*float64".into(),
        TypeRef::Primitive { name } if name == "boolean" => "*bool".into(),
        TypeRef::Named { name } => format!("*{}", sanitize_exported_identifier(name)),
        _ => go_type_ref(type_ref),
    }
}

pub(crate) fn go_result_type(type_ref: &TypeRef) -> String {
    if returns_pointer_result(type_ref) {
        format!("*{}", go_decode_type(type_ref))
    } else {
        go_decode_type(type_ref)
    }
}

pub(crate) fn go_decode_type(type_ref: &TypeRef) -> String {
    match type_ref {
        TypeRef::Named { name } => sanitize_exported_identifier(name),
        _ => go_type_ref(type_ref),
    }
}

pub(crate) fn returns_pointer_result(type_ref: &TypeRef) -> bool {
    matches!(type_ref, TypeRef::Named { .. })
}

pub(crate) fn returns_nil_on_error(type_ref: &TypeRef) -> bool {
    match type_ref {
        TypeRef::Named { .. } | TypeRef::Array { .. } | TypeRef::Map { .. } => true,
        TypeRef::Primitive { name } => name == "binary",
        TypeRef::Union { .. } => false,
    }
}

pub(crate) fn go_http_method(method: HttpMethod) -> &'static str {
    match method {
        HttpMethod::Get => "http.MethodGet",
        HttpMethod::Post => "http.MethodPost",
        HttpMethod::Put => "http.MethodPut",
        HttpMethod::Patch => "http.MethodPatch",
        HttpMethod::Delete => "http.MethodDelete",
    }
}
