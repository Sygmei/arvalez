use std::borrow::Cow;
use std::path::Path;

use anyhow::{Result, anyhow};

use crate::document::{OpenApiDocument, Swagger2Document, OpenApi3Document};
use crate::source::{LoadedOpenApiDocument, OpenApiSource, SourceFormat};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpenApiVersion {
    Swagger2,
    OpenApi3,
}

fn detect_openapi_version(raw: &str) -> OpenApiVersion {
    // Scan the first 4 KB — enough to encounter the version field in any real document.
    let sample = &raw[..raw.len().min(4096)];
    if sample.contains("\"swagger\"")
        || sample.starts_with("swagger:")
        || sample.contains("\nswagger:")
    {
        OpenApiVersion::Swagger2
    } else {
        OpenApiVersion::OpenApi3
    }
}

fn deserialize_json<T>(path: &Path, raw: &str) -> Result<T>
where
    T: for<'de> serde::Deserialize<'de>,
{
    let mut deserializer = serde_json::Deserializer::from_str(raw);
    serde_path_to_error::deserialize(&mut deserializer).map_err(|error| {
        let schema_path = error.path().to_string();
        let inner = error.into_inner();
        let line = inner.line();
        let column = inner.column();
        let message = inner.to_string();
        anyhow!(format_openapi_deserialize_error(
            "JSON",
            path,
            raw,
            if schema_path.is_empty() {
                None
            } else {
                Some(schema_path.as_str())
            },
            line,
            column,
            &message,
        ))
    })
}

fn deserialize_yaml<T>(path: &Path, raw: &str, sanitized: &str) -> Result<T>
where
    T: for<'de> serde::Deserialize<'de>,
{
    let deserializer = serde_yaml::Deserializer::from_str(sanitized);
    serde_path_to_error::deserialize(deserializer).map_err(|error| {
        let schema_path = error.path().to_string();
        let inner = error.into_inner();
        let (line, column) = inner
            .location()
            .map(|location| (location.line(), location.column()))
            .unwrap_or((0, 0));
        anyhow!(format_openapi_deserialize_error(
            "YAML",
            path,
            raw,
            if schema_path.is_empty() {
                None
            } else {
                Some(schema_path.as_str())
            },
            line,
            column,
            &inner.to_string(),
        ))
    })
}

pub(crate) fn parse_json_openapi_document(path: &Path, raw: &str) -> Result<LoadedOpenApiDocument> {
    let document = if detect_openapi_version(raw) == OpenApiVersion::Swagger2 {
        OpenApiDocument::from(deserialize_json::<Swagger2Document>(path, raw)?)
    } else {
        OpenApiDocument::from(deserialize_json::<OpenApi3Document>(path, raw)?)
    };
    Ok(LoadedOpenApiDocument {
        document,
        source: OpenApiSource::new(SourceFormat::Json, raw.to_owned()),
    })
}

pub(crate) fn parse_yaml_openapi_document(path: &Path, raw: &str) -> Result<LoadedOpenApiDocument> {
    let sanitized = sanitize_yaml_for_parser(raw);
    let document = if detect_openapi_version(raw) == OpenApiVersion::Swagger2 {
        OpenApiDocument::from(deserialize_yaml::<Swagger2Document>(
            path,
            raw,
            sanitized.as_ref(),
        )?)
    } else {
        OpenApiDocument::from(deserialize_yaml::<OpenApi3Document>(
            path,
            raw,
            sanitized.as_ref(),
        )?)
    };
    Ok(LoadedOpenApiDocument {
        document,
        source: OpenApiSource::new(SourceFormat::Yaml, raw.to_owned()),
    })
}

fn format_openapi_deserialize_error(
    format_name: &str,
    path: &Path,
    raw: &str,
    schema_path: Option<&str>,
    line: usize,
    column: usize,
    message: &str,
) -> String {
    let mut rendered = format!(
        "failed to parse {format_name} OpenAPI document `{}`",
        path.display()
    );
    rendered.push_str("\nCaused by:");

    if let Some(schema_path) = schema_path {
        rendered.push_str(&format!(
            "\n  schema mismatch at `{schema_path}`: {message}"
        ));
    } else {
        rendered.push_str(&format!("\n  {message}"));
    }

    if line > 0 && column > 0 {
        rendered.push_str(&format!("\n  location: line {line}, column {column}"));
        if let Some(source_line) = raw.lines().nth(line.saturating_sub(1)) {
            rendered.push_str(&format!("\n  source: {source_line}"));
            rendered.push_str(&format!(
                "\n          {}^",
                " ".repeat(column.saturating_sub(1))
            ));
        }
    }

    rendered.push_str(
        "\n  note: this usually means the document is valid JSON/YAML, but an OpenAPI field had an unexpected shape.",
    );
    rendered
}

pub(crate) fn sanitize_yaml_for_parser(raw: &str) -> Cow<'_, str> {
    // U+2028 LINE SEPARATOR and U+2029 PARAGRAPH SEPARATOR are treated as
    // newlines by serde_yaml (YAML 1.1 §4.2). When they appear inside a block
    // scalar they create implicit line breaks whose "continuation" indentation
    // is wrong, prematurely ending the block scalar and then causing a parse
    // error on the next real YAML line. Replace them with regular spaces so
    // the block scalar content is preserved verbatim.
    //
    // C1 control codes (U+0080–U+009F) are forbidden by YAML 1.2. They
    // sometimes appear as artifacts of double-UTF-8 encoding (e.g. U+2019 '
    // double-encoded produces the bytes C2 80 / C2 99 which decode to U+0080
    // and U+0099). Strip them so the parser doesn't reject the document.
    let needs_unicode_fix = raw.contains('\u{2028}') || raw.contains('\u{2029}');
    let needs_c1_fix = raw.chars().any(|c| ('\u{0080}'..='\u{009F}').contains(&c));
    if !raw.contains('\t') && !needs_unicode_fix && !needs_c1_fix {
        return Cow::Borrowed(raw);
    }

    let mut changed = false;
    let mut normalized = String::with_capacity(raw.len());

    for segment in raw.split_inclusive('\n') {
        let line = segment.strip_suffix('\n').unwrap_or(segment);
        let has_newline = segment.len() != line.len();
        if line.contains('\t') && line.chars().all(|ch| matches!(ch, ' ' | '\t')) {
            changed = true;
        } else {
            // Apply both fixes in one pass when either is needed.
            let has_sep = line.contains('\u{2028}') || line.contains('\u{2029}');
            let has_c1 = line.chars().any(|c| ('\u{0080}'..='\u{009F}').contains(&c));
            if has_sep || has_c1 {
                let fixed: String = line
                    .chars()
                    .map(|c| match c {
                        // Unicode line/paragraph separators → space (preserve blank-line intent)
                        '\u{2028}' | '\u{2029}' => ' ',
                        // C1 control codes → strip by replacing with nothing (handled below)
                        c if ('\u{0080}'..='\u{009F}').contains(&c) => '\0',
                        c => c,
                    })
                    .filter(|&c| c != '\0')
                    .collect();
                normalized.push_str(&fixed);
                changed = true;
            } else {
                normalized.push_str(line);
            }
        }

        if has_newline {
            normalized.push('\n');
        }
    }

    if changed {
        Cow::Owned(normalized)
    } else {
        Cow::Borrowed(raw)
    }
}