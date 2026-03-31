pub(crate) fn sanitize_package_name(name: &str) -> String {
    let mut out = split_words(name).join("");
    if out.is_empty() {
        out = "client".into();
    }
    if out.chars().next().is_some_and(|ch| ch.is_ascii_digit()) {
        out.insert(0, 'x');
    }
    if is_go_keyword(&out) {
        out.push_str("pkg");
    }
    out.to_ascii_lowercase()
}

pub(crate) fn sanitize_exported_identifier(name: &str) -> String {
    let mut out = String::new();
    for word in split_words(name) {
        let mut chars = word.chars();
        if let Some(first) = chars.next() {
            out.extend(first.to_uppercase());
            out.push_str(chars.as_str());
        }
    }
    if out.is_empty() {
        out = "Generated".into();
    }
    if out.chars().next().is_some_and(|ch| ch.is_ascii_digit()) {
        out.insert(0, 'X');
    }
    if is_go_keyword(&out.to_ascii_lowercase()) {
        out.push('_');
    }
    out
}

pub(crate) fn sanitize_identifier(name: &str) -> String {
    let words = split_words(name);
    let mut out = if words.is_empty() {
        "value".into()
    } else {
        let mut iter = words.into_iter();
        let mut result = iter.next().unwrap_or_else(|| "value".into());
        for word in iter {
            let mut chars = word.chars();
            if let Some(first) = chars.next() {
                result.extend(first.to_uppercase());
                result.push_str(chars.as_str());
            }
        }
        result
    };

    if out.chars().next().is_some_and(|ch| ch.is_ascii_digit()) {
        out.insert(0, 'x');
    }
    if is_go_keyword(&out) {
        out.push('_');
    }
    out
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

pub(crate) fn is_go_keyword(value: &str) -> bool {
    matches!(
        value,
        "break"
            | "default"
            | "func"
            | "interface"
            | "select"
            | "case"
            | "defer"
            | "go"
            | "map"
            | "struct"
            | "chan"
            | "else"
            | "goto"
            | "package"
            | "switch"
            | "const"
            | "fallthrough"
            | "if"
            | "range"
            | "type"
            | "continue"
            | "for"
            | "import"
            | "return"
            | "var"
    )
}
