pub fn sanitize_class_name(name: &str) -> String {
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

pub fn sanitize_identifier(name: &str) -> String {
    let words = split_words(name);
    sanitize_words_as_identifier(words)
}

pub fn sanitize_subsection_name(name: &str) -> String {
    let words = merge_subsection_words(split_words(name));
    sanitize_words_as_identifier(words)
}

fn sanitize_words_as_identifier(words: Vec<String>) -> String {
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

fn merge_subsection_words(words: Vec<String>) -> Vec<String> {
    let mut merged = Vec::new();
    let mut index = 0;

    while index < words.len() {
        if let Some(next) = words.get(index + 1) {
            let current = &words[index];
            if is_split_acronym_plural(current, next) || is_oauth_pair(current, next) {
                merged.push(format!("{current}{next}"));
                index += 2;
                continue;
            }
        }

        merged.push(words[index].clone());
        index += 1;
    }

    merged
}

fn is_split_acronym_plural(left: &str, right: &str) -> bool {
    left.len() > 1
        && left
            .chars()
            .all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit())
        && {
            let mut chars = right.chars();
            matches!(
                (chars.next(), chars.next(), chars.next()),
                (Some(first), Some('s'), None) if first.is_ascii_uppercase()
            )
        }
}

fn is_oauth_pair(left: &str, right: &str) -> bool {
    left == "O"
        && right
            .strip_prefix("Auth")
            .is_some_and(|suffix| suffix.chars().all(|ch| ch.is_ascii_digit()))
}

pub(crate) use arvalez_target_core::split_words;

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
            | "match"
            | "none"
            | "nonlocal"
            | "not"
            | "or"
            | "pass"
            | "raise"
            | "return"
            | "true"
            | "try"
            | "type"
            | "case"
            | "while"
            | "with"
            | "yield"
    )
}
