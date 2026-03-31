use arvalez_ir::{Attributes, HttpMethod};
use serde_json::{Value, json};
use crate::document::OperationSpec;

pub(crate) fn fallback_operation_name(method: HttpMethod, path: &str) -> String {
    to_snake_case(&format!("{} {}", method_key(method), path))
}

pub(crate) fn method_key(method: HttpMethod) -> &'static str {
    match method {
        HttpMethod::Get => "get",
        HttpMethod::Post => "post",
        HttpMethod::Put => "put",
        HttpMethod::Patch => "patch",
        HttpMethod::Delete => "delete",
    }
}

pub(crate) fn operation_attributes(spec: &OperationSpec) -> Attributes {
    let mut attributes = Attributes::default();
    if let Some(summary) = &spec.summary {
        attributes.insert("summary".into(), Value::String(summary.clone()));
    }
    if !spec.tags.is_empty() {
        attributes.insert("tags".into(), json!(spec.tags));
    }
    attributes
}

pub(crate) fn json_pointer_key(input: &str) -> String {
    input.replace('~', "~0").replace('/', "~1")
}

pub(crate) fn to_pascal_case(input: &str) -> String {
    let mut output = String::new();
    for part in split_words(input) {
        let mut chars = part.chars();
        if let Some(first) = chars.next() {
            output.extend(first.to_uppercase());
            output.push_str(chars.as_str());
        }
    }
    if output.is_empty() {
        "InlineModel".into()
    } else {
        output
    }
}

pub(crate) fn to_snake_case(input: &str) -> String {
    let parts = split_words(input);
    if parts.is_empty() {
        return "value".into();
    }
    parts.join("_").to_lowercase()
}

pub(crate) fn split_words(input: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();

    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() {
            if ch.is_uppercase() && !current.is_empty() {
                words.push(current.clone());
                current.clear();
            }
            current.push(ch.to_ascii_lowercase());
        } else if !current.is_empty() {
            words.push(current.clone());
            current.clear();
        }
    }

    if !current.is_empty() {
        words.push(current);
    }

    words
}