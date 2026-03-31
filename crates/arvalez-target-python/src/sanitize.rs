pub(crate) fn sanitize_class_name(name: &str) -> String {
    let mut out = String::new();
    for part in split_words(name) {
        if part.len() > 1
            && part
                .chars()
                .all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit())
            && part.chars().any(|ch| ch.is_ascii_uppercase())
        {
            out.push_str(&part);
        } else {
            let mut chars = part.chars();
            if let Some(first) = chars.next() {
                out.extend(first.to_uppercase());
                out.push_str(&chars.as_str().to_ascii_lowercase());
            }
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
        words
            .into_iter()
            .map(|word| word.to_ascii_lowercase())
            .collect::<Vec<_>>()
            .join("_")
    };
    if candidate
        .chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_digit())
    {
        candidate.insert(0, '_');
    }
    if is_python_keyword(&candidate) {
        candidate.push('_');
    }
    candidate
}

pub(crate) fn sanitize_enum_member_name(value: &str) -> String {
    let mut candidate = String::new();
    let mut last_was_separator = false;
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            candidate.push(ch.to_ascii_uppercase());
            last_was_separator = false;
        } else if !last_was_separator && !candidate.is_empty() {
            candidate.push('_');
            last_was_separator = true;
        }
    }
    while candidate.ends_with('_') {
        candidate.pop();
    }
    if candidate.is_empty() {
        candidate = "VALUE".into();
    }
    if candidate
        .chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_digit())
    {
        candidate.insert(0, '_');
    }
    if is_python_keyword(&candidate.to_ascii_lowercase()) {
        candidate.push('_');
    }
    candidate
}

pub(crate) fn split_words(input: &str) -> Vec<String> {
    let chars = input.chars().collect::<Vec<_>>();
    let mut words = Vec::new();
    let mut current = String::new();
    for (index, ch) in chars.iter().copied().enumerate() {
        if ch.is_ascii_alphanumeric() {
            let previous = index.checked_sub(1).and_then(|value| chars.get(value)).copied();
            let next = chars.get(index + 1).copied();
            let next_next = chars.get(index + 2).copied();
            let should_split = !current.is_empty()
                && previous.is_some_and(|prev| {
                    (prev.is_ascii_lowercase() && ch.is_ascii_uppercase())
                        || (prev.is_ascii_digit() && ch.is_ascii_alphabetic())
                        || (prev.is_ascii_uppercase()
                            && ch.is_ascii_uppercase()
                            && (current.len() > 1
                                || (current.len() == 1
                                    && next_next
                                        .is_some_and(|candidate| candidate.is_ascii_lowercase())))
                            && next.is_some_and(|candidate| candidate.is_ascii_lowercase()))
                });
            if should_split {
                words.push(current.clone());
                current.clear();
            }
            current.push(ch);
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

pub(crate) fn is_python_keyword(value: &str) -> bool {
    matches!(
        value,
        "and"
            | "as"
            | "assert"
            | "async"
            | "await"
            | "break"
            | "class"
            | "continue"
            | "def"
            | "del"
            | "elif"
            | "else"
            | "except"
            | "false"
            | "finally"
            | "for"
            | "from"
            | "global"
            | "if"
            | "import"
            | "in"
            | "is"
            | "lambda"
            | "none"
            | "nonlocal"
            | "not"
            | "or"
            | "pass"
            | "raise"
            | "return"
            | "true"
            | "try"
            | "while"
            | "with"
            | "yield"
    )
}
