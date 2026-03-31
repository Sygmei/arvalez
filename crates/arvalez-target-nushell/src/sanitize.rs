/// Split a mixed-case / snake_case / kebab-case identifier into lowercase words.
pub(crate) fn split_words(name: &str) -> Vec<String> {
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

/// Nushell command / subcommand name: kebab-case (`get-widget`).
pub(crate) fn sanitize_command_name(name: &str) -> String {
    let words = split_words(name);
    if words.is_empty() {
        return "command".into();
    }
    words.join("-")
}

/// Nushell flag name: kebab-case (`--include-count`).
pub(crate) fn sanitize_flag_name(name: &str) -> String {
    let words = split_words(name);
    if words.is_empty() {
        return "flag".into();
    }
    words.join("-")
}

/// Nushell variable / positional parameter name: snake_case (`widget_id`).
pub(crate) fn sanitize_variable_name(name: &str) -> String {
    let words = split_words(name);
    let candidate = if words.is_empty() {
        "value".into()
    } else {
        words.join("_")
    };
    if candidate.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        format!("_{candidate}")
    } else if is_nushell_keyword(&candidate) {
        format!("{candidate}_")
    } else {
        candidate
    }
}

/// Nushell record field name: kebab-case string (used in record literals).
pub(crate) fn sanitize_field_name(name: &str) -> String {
    let words = split_words(name);
    if words.is_empty() {
        return "field".into();
    }
    let candidate = words.join("_");
    if is_nushell_keyword(&candidate) {
        format!("{candidate}_")
    } else {
        candidate
    }
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
