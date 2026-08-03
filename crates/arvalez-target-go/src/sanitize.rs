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
        out.push_str(&exported_word(&word));
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
            result.push_str(&exported_word(&word));
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

fn exported_word(word: &str) -> String {
    if is_initialism(word) {
        return word.to_ascii_uppercase();
    }
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

fn is_initialism(word: &str) -> bool {
    matches!(
        word,
        "api"
            | "ascii"
            | "cpu"
            | "css"
            | "dns"
            | "eof"
            | "guid"
            | "html"
            | "http"
            | "https"
            | "id"
            | "ip"
            | "json"
            | "qps"
            | "ram"
            | "rpc"
            | "sla"
            | "smtp"
            | "sql"
            | "ssh"
            | "tcp"
            | "tls"
            | "ttl"
            | "udp"
            | "ui"
            | "uid"
            | "uri"
            | "url"
            | "utf8"
            | "uuid"
            | "vm"
            | "xml"
            | "xmpp"
            | "xsrf"
            | "xss"
    )
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
