pub(crate) fn sanitize_type_name(name: &str) -> String {
    let mut out = String::new();
    for part in split_words(name) {
        let mut chars = part.chars();
        if let Some(first) = chars.next() {
            out.extend(first.to_uppercase());
            out.push_str(chars.as_str());
        }
    }
    if out.is_empty() {
        "GeneratedModel".into()
    } else if out.chars().next().is_some_and(|ch| ch.is_ascii_digit()) {
        format!("_{out}")
    } else {
        out
    }
}

pub(crate) fn sanitize_identifier(name: &str) -> String {
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

    if candidate
        .chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_digit())
    {
        candidate.insert(0, '_');
    }
    if is_typescript_keyword(&candidate) {
        candidate.push('_');
    }
    candidate
}

pub(crate) fn sanitize_tag_property_name(name: &str) -> String {
    let property = split_words(name).join("_");
    if property.is_empty() {
        "default".into()
    } else if is_typescript_keyword(&property) {
        format!("{property}_")
    } else {
        property
    }
}

pub(crate) fn render_property_name(name: &str) -> String {
    if is_valid_typescript_identifier(name) && !is_typescript_keyword(name) {
        name.into()
    } else {
        format!("{name:?}")
    }
}

pub(crate) fn is_valid_typescript_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    match chars.next() {
        Some(ch) if ch == '_' || ch == '$' || ch.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|ch| ch == '_' || ch == '$' || ch.is_ascii_alphanumeric())
}

pub(crate) fn sanitize_doc_text(value: &str) -> String {
    value.replace("*/", "*\\/")
}

pub(crate) use arvalez_target_core::split_words;

pub(crate) fn is_typescript_keyword(value: &str) -> bool {
    matches!(
        value,
        "break"
            | "case"
            | "catch"
            | "class"
            | "const"
            | "continue"
            | "debugger"
            | "default"
            | "delete"
            | "do"
            | "else"
            | "enum"
            | "export"
            | "extends"
            | "false"
            | "finally"
            | "for"
            | "function"
            | "if"
            | "import"
            | "in"
            | "instanceof"
            | "new"
            | "null"
            | "return"
            | "super"
            | "switch"
            | "this"
            | "throw"
            | "true"
            | "try"
            | "typeof"
            | "var"
            | "void"
            | "while"
            | "with"
            | "yield"
    )
}
