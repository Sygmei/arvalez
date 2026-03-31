use std::borrow::Cow;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs,
    path::Path,
    sync::OnceLock,
    time::Instant,
};

use anyhow::{Context, Result, anyhow, bail};
use arvalez_ir::{
    Attributes, CoreIr, Field, HttpMethod, Model, Operation, Parameter, ParameterLocation,
    RequestBody, Response, SourceRef, TypeRef, validate_ir,
};
use indexmap::IndexMap;
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Debug, Clone, Copy, Default)]
pub struct LoadOpenApiOptions {
    pub ignore_unhandled: bool,
    pub emit_timings: bool,
}

/// Structured diagnostic emitted by the OpenAPI importer. Implements
/// [`std::error::Error`] so it can be stored inside [`anyhow::Error`],
/// enabling callers to downcast and inspect the structured data instead of
/// parsing the human-readable error string.
#[derive(Debug, Clone)]
pub struct OpenApiDiagnostic {
    /// Machine-readable classification of the issue.
    pub kind: DiagnosticKind,
    /// JSON pointer into the document where the issue was detected.
    pub pointer: Option<String>,
    /// A snippet of the document at the `pointer` location.
    pub source_preview: Option<String>,
    /// Human-readable context when there is no pointer (e.g. `"parameter \`foo\`"`).
    pub context: Option<String>,
    /// Approximate 1-based source line for the node at `pointer`, when resolvable.
    pub line: Option<usize>,
}

impl OpenApiDiagnostic {
    pub fn from_pointer(
        kind: DiagnosticKind,
        pointer: impl Into<String>,
        source_preview: Option<String>,
        line: Option<usize>,
    ) -> Self {
        OpenApiDiagnostic {
            kind,
            pointer: Some(pointer.into()),
            source_preview,
            context: None,
            line,
        }
    }

    pub fn from_named_context(kind: DiagnosticKind, context: impl Into<String>) -> Self {
        OpenApiDiagnostic {
            kind,
            pointer: None,
            source_preview: None,
            context: Some(context.into()),
            line: None,
        }
    }

    pub fn simple(kind: DiagnosticKind) -> Self {
        OpenApiDiagnostic {
            kind,
            pointer: None,
            source_preview: None,
            context: None,
            line: None,
        }
    }

    /// Returns the human-readable note text for this diagnostic, if any.
    pub fn note(&self) -> Option<&str> {
        let note = self.kind.note_text();
        if note.is_empty() { None } else { Some(note) }
    }

    /// Returns the corpus `(kind, feature)` classification for this diagnostic.
    ///
    /// This is the canonical mapping from [`DiagnosticKind`] to the string
    /// identifiers used in corpus reports.  Keeping it here means adding a new
    /// variant produces a compile error at the definition site.
    pub fn classify(&self) -> (&'static str, String) {
        match &self.kind {
            DiagnosticKind::UnknownSchemaKeyword { keyword } => {
                ("unsupported_schema_keyword", keyword.clone())
            }
            DiagnosticKind::UnsupportedSchemaKeyword { keyword } => (
                Self::unsupported_kind_for_pointer(self.pointer.as_deref(), keyword),
                keyword.clone(),
            ),
            DiagnosticKind::UnsupportedSchemaType { schema_type } => {
                ("unsupported_schema_type", schema_type.clone())
            }
            DiagnosticKind::UnsupportedSchemaShape => (
                "unsupported_schema_shape",
                self.pointer
                    .as_deref()
                    .map(diagnostic_pointer_tail)
                    .unwrap_or_else(|| "schema_shape".into()),
            ),
            DiagnosticKind::UnsupportedReference { reference } => {
                ("unsupported_reference", categorize_reference(reference))
            }
            DiagnosticKind::AllOfRecursiveCycle { .. } => {
                ("unsupported_all_of_merge", "recursive_cycle".into())
            }
            DiagnosticKind::RecursiveParameterCycle { .. } => (
                "invalid_openapi_document",
                "recursive_parameter_cycle".into(),
            ),
            DiagnosticKind::RecursiveRequestBodyCycle { .. } => (
                "invalid_openapi_document",
                "recursive_request_body_cycle".into(),
            ),
            DiagnosticKind::IncompatibleAllOfField { field } => {
                ("unsupported_all_of_merge", field.clone())
            }
            DiagnosticKind::EmptyRequestBodyContent => {
                ("unsupported_request_body_shape", "empty_content".into())
            }
            DiagnosticKind::EmptyParameterName { .. } => {
                ("invalid_openapi_document", "empty_parameter_name".into())
            }
            DiagnosticKind::EmptyPropertyKey { .. } => {
                ("invalid_openapi_document", "empty_property_key".into())
            }
            DiagnosticKind::ParameterMissingSchema { name } => (
                "invalid_openapi_document",
                normalize_diagnostic_feature(name),
            ),
            DiagnosticKind::UnsupportedParameterLocation { name } => (
                "invalid_openapi_document",
                normalize_diagnostic_feature(name),
            ),
            DiagnosticKind::MultipleRequestBodyDeclarations { .. } => (
                "invalid_openapi_document",
                "multiple_request_body_declarations".into(),
            ),
            DiagnosticKind::BodyParameterMissingSchema { name } => (
                "invalid_openapi_document",
                normalize_diagnostic_feature(name),
            ),
            DiagnosticKind::FormDataParameterMissingSchema { name } => (
                "invalid_openapi_document",
                normalize_diagnostic_feature(name),
            ),
        }
    }

    pub fn unsupported_kind_for_pointer(pointer: Option<&str>, feature: &str) -> &'static str {
        if matches!(
            feature,
            "allOf" | "anyOf" | "oneOf" | "not" | "discriminator" | "const"
        ) {
            return "unsupported_schema_keyword";
        }
        match pointer {
            Some(p)
                if p.contains("/components/schemas/")
                    || p.contains("/properties/")
                    || p.ends_with("/schema")
                    || p.contains("/items/") =>
            {
                "unsupported_schema_keyword"
            }
            Some(p) if p.contains("/parameters/") => "unsupported_parameter_feature",
            Some(p) if p.contains("/responses/") => "unsupported_response_feature",
            Some(p) if p.contains("/requestBody/") => "unsupported_request_body_feature",
            _ => "unsupported_feature",
        }
    }
}

pub fn categorize_reference(reference: &str) -> String {
    // External references (http/https/relative paths without #) are their own category.
    if !reference.starts_with('#') {
        return "external".into();
    }
    // Strip `#/` and split into path segments, ignoring percent-encoded path globs
    // and numeric indices so only structural keywords remain.
    let inner = reference.strip_prefix("#/").unwrap_or("");
    let structural: Vec<&str> = inner
        .split('/')
        .filter(|s| !s.is_empty() && !s.chars().all(|c| c.is_ascii_digit()) && !s.contains('~') && !s.contains('%'))
        .take(2)
        .collect();
    if structural.is_empty() {
        return "unknown".into();
    }
    structural.join("_")
}

pub fn diagnostic_pointer_tail(pointer: &str) -> String {
    pointer
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .map(normalize_diagnostic_feature)
        .unwrap_or_else(|| "schema_shape".into())
}

pub fn normalize_diagnostic_feature(value: &str) -> String {
    value
        .replace("~1", "/")
        .replace("~0", "~")
        .replace('.', "_")
        .replace('/', "_")
        .replace('`', "")
}

impl std::fmt::Display for OpenApiDiagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = self.kind.message_text();
        let note = self.kind.note_text();

        if let Some(pointer) = &self.pointer {
            write!(f, "OpenAPI document issue\nCaused by:\n  {message}")?;
            write!(f, "\n  location: {pointer}")?;
            if let Some(preview) = &self.source_preview {
                write!(f, "\n  preview:")?;
                for line in preview.lines() {
                    write!(f, "\n    {line}")?;
                }
            }
            if !note.is_empty() {
                write!(f, "\n  note: {note}")?;
            }
        } else if let Some(context) = &self.context {
            write!(f, "{context}: {message}")?;
            if !note.is_empty() {
                write!(f, "\nnote: {note}")?;
            }
        } else {
            write!(f, "{message}")?;
            if !note.is_empty() {
                write!(f, "\nnote: {note}")?;
            }
        }
        Ok(())
    }
}

impl std::error::Error for OpenApiDiagnostic {}

/// Machine-readable classification of an [`OpenApiDiagnostic`].
#[derive(Debug, Clone)]
pub enum DiagnosticKind {
    // ── Schema keyword issues ────────────────────────────────────────────────
    /// An unrecognised keyword was found in a schema object.
    UnknownSchemaKeyword { keyword: String },
    /// A recognised but unsupported schema keyword was found.
    UnsupportedSchemaKeyword { keyword: String },
    /// A schema declared a type string that Arvalez cannot map.
    UnsupportedSchemaType { schema_type: String },
    /// A schema's overall shape could not be mapped to a type.
    UnsupportedSchemaShape,
    // ── Reference issues ─────────────────────────────────────────────────────
    /// A `$ref` value points to a location Arvalez cannot resolve.
    UnsupportedReference { reference: String },
    /// A `$ref` chain inside `allOf` (or object view collection) is cyclic.
    AllOfRecursiveCycle { reference: String },
    /// A parameter `$ref` is cyclic.
    RecursiveParameterCycle { reference: String },
    /// A `requestBody` `$ref` is cyclic.
    RecursiveRequestBodyCycle { reference: String },
    // ── allOf merge issues ───────────────────────────────────────────────────
    /// Two `allOf` members declare incompatible values for the same keyword.
    IncompatibleAllOfField { field: String },
    // ── Structural document issues ───────────────────────────────────────────
    /// A `requestBody` object has an empty `content` map.
    EmptyRequestBodyContent,
    /// A parameter's `name` field is the empty string.
    EmptyParameterName { counter: usize },
    /// An object schema property key is the empty string.
    EmptyPropertyKey { counter: usize },
    /// A non-body parameter has neither a `schema` nor a `type`.
    ParameterMissingSchema { name: String },
    /// A parameter uses a location (`in`) value Arvalez cannot handle.
    UnsupportedParameterLocation { name: String },
    /// An operation has multiple conflicting request-body sources.
    MultipleRequestBodyDeclarations { note: String },
    /// A Swagger 2 `in: body` parameter has no `schema`.
    BodyParameterMissingSchema { name: String },
    /// A Swagger 2 `in: formData` parameter has no schema/type.
    FormDataParameterMissingSchema { name: String },
}

impl DiagnosticKind {
    fn message_text(&self) -> String {
        match self {
            Self::UnknownSchemaKeyword { keyword } => format!("unknown schema keyword `{keyword}`"),
            Self::UnsupportedSchemaKeyword { keyword } => {
                format!("`{keyword}` is not supported yet")
            }
            Self::UnsupportedSchemaType { schema_type } => {
                format!("unsupported schema type `{schema_type}`")
            }
            Self::UnsupportedSchemaShape => "schema shape is not supported yet".into(),
            Self::UnsupportedReference { reference } => {
                format!("unsupported reference `{reference}`")
            }
            Self::AllOfRecursiveCycle { reference } => {
                format!("`allOf` contains a recursive reference cycle involving `{reference}`")
            }
            Self::RecursiveParameterCycle { reference } => {
                format!("parameter reference contains a recursive cycle involving `{reference}`")
            }
            Self::RecursiveRequestBodyCycle { reference } => {
                format!("request body reference contains a recursive cycle involving `{reference}`")
            }
            Self::IncompatibleAllOfField { field } => {
                format!("`allOf` contains incompatible `{field}` declarations")
            }
            Self::EmptyRequestBodyContent => "request body has no content entries".into(),
            Self::EmptyParameterName { counter } => {
                format!("parameter #{counter} has an empty name")
            }
            Self::EmptyPropertyKey { counter } => format!("property #{counter} has an empty name"),
            Self::ParameterMissingSchema { .. } => "parameter has no schema or type".into(),
            Self::UnsupportedParameterLocation { .. } => "unsupported parameter location".into(),
            Self::MultipleRequestBodyDeclarations { .. } => {
                "multiple request body declarations are not supported".into()
            }
            Self::BodyParameterMissingSchema { .. } => "body parameter has no schema".into(),
            Self::FormDataParameterMissingSchema { .. } => {
                "formData parameter has no schema or type".into()
            }
        }
    }

    fn note_text(&self) -> &str {
        match self {
            Self::UnknownSchemaKeyword { .. }
            | Self::UnsupportedSchemaKeyword { .. }
            | Self::UnsupportedSchemaType { .. }
            | Self::UnsupportedSchemaShape
            | Self::AllOfRecursiveCycle { .. }
            | Self::IncompatibleAllOfField { .. }
            | Self::EmptyParameterName { .. }
            | Self::EmptyPropertyKey { .. } => {
                "Use `--ignore-unhandled` to turn this into a warning while keeping generation going."
            }
            Self::EmptyRequestBodyContent => {
                "Arvalez defaulted this request body to `application/octet-stream` with an untyped payload."
            }
            Self::ParameterMissingSchema { .. } => {
                "Arvalez currently expects non-body parameters to declare either `schema` (OpenAPI 3) or `type` (Swagger 2)."
            }
            Self::UnsupportedParameterLocation { .. } => {
                "Arvalez currently supports path, query, header, and cookie parameters here."
            }
            Self::MultipleRequestBodyDeclarations { note } => note.as_str(),
            Self::BodyParameterMissingSchema { .. } => {
                "Swagger 2 `in: body` parameters must declare a `schema`."
            }
            Self::FormDataParameterMissingSchema { .. } => {
                "Swagger 2 `in: formData` parameters must declare either a `type` or a `schema`."
            }
            Self::RecursiveParameterCycle { .. } => {
                "Arvalez only supports acyclic parameter references."
            }
            Self::RecursiveRequestBodyCycle { .. } => {
                "Arvalez only supports acyclic local `requestBody` references."
            }
            Self::UnsupportedReference { .. } => "",
        }
    }
}

#[derive(Debug, Clone)]
pub struct OpenApiLoadResult {
    pub ir: CoreIr,
    pub warnings: Vec<OpenApiDiagnostic>,
}

pub fn load_openapi_to_ir(path: impl AsRef<Path>) -> Result<CoreIr> {
    Ok(load_openapi_to_ir_with_options(path, LoadOpenApiOptions::default())?.ir)
}

pub fn load_openapi_to_ir_with_options(
    path: impl AsRef<Path>,
    options: LoadOpenApiOptions,
) -> Result<OpenApiLoadResult> {
    let path = path.as_ref();
    let raw = measure_openapi_phase(options.emit_timings, "openapi_read", || {
        fs::read_to_string(path)
            .with_context(|| format!("failed to read OpenAPI document `{}`", path.display()))
    })?;

    let loaded = measure_openapi_phase(options.emit_timings, "openapi_parse", || {
        match path.extension().and_then(|ext| ext.to_str()) {
            Some("yaml") | Some("yml") => parse_yaml_openapi_document(path, &raw),
            _ => parse_json_openapi_document(path, &raw),
        }
    })?;

    OpenApiImporter::new(loaded.document, loaded.source, options).build_ir()
}

fn measure_openapi_phase<T, F>(enabled: bool, label: &str, task: F) -> Result<T>
where
    F: FnOnce() -> Result<T>,
{
    if enabled {
        eprintln!("timing: starting {label}");
    }
    let started = Instant::now();
    let value = task();
    if enabled {
        eprintln!(
            "timing: {:<20} {}",
            label,
            format_duration(started.elapsed())
        );
    }
    value
}

fn format_duration(duration: std::time::Duration) -> String {
    let micros = duration.as_micros();
    if micros < 1_000 {
        format!("{micros}us")
    } else if micros < 1_000_000 {
        format!("{}ms", duration.as_millis())
    } else {
        format!("{:.2}s", duration.as_secs_f64())
    }
}

fn deserialize_paths_map<'de, D>(
    deserializer: D,
) -> std::result::Result<BTreeMap<String, PathItem>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    // Use a proper Visitor so the `serde_path_to_error`-wrapped deserializer
    // stays in scope when each PathItem is deserialized.  Deserializing via an
    // intermediate `BTreeMap<String, Value>` then `serde_json::from_value` would
    // spin up a fresh deserialization context, losing all path tracking and
    // causing errors to be reported as just `paths` instead of the full path.
    struct PathsMapVisitor;

    impl<'de> serde::de::Visitor<'de> for PathsMapVisitor {
        type Value = BTreeMap<String, PathItem>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a map of path items")
        }

        fn visit_map<A>(self, mut map: A) -> std::result::Result<Self::Value, A::Error>
        where
            A: serde::de::MapAccess<'de>,
        {
            let mut result = BTreeMap::new();
            while let Some(key) = map.next_key::<String>()? {
                if key.starts_with("x-") {
                    // Drain the value without deserializing it.
                    map.next_value::<serde::de::IgnoredAny>()?;
                } else {
                    // Deserialize directly as PathItem so that serde_path_to_error
                    // can track the path key → PathItem fields.
                    let value = map.next_value::<PathItem>()?;
                    result.insert(key, value);
                }
            }
            Ok(result)
        }
    }

    deserializer.deserialize_map(PathsMapVisitor)
}

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

fn parse_json_openapi_document(path: &Path, raw: &str) -> Result<LoadedOpenApiDocument> {
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

fn parse_yaml_openapi_document(path: &Path, raw: &str) -> Result<LoadedOpenApiDocument> {
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

fn sanitize_yaml_for_parser(raw: &str) -> Cow<'_, str> {
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

struct OpenApiImporter {
    document: OpenApiDocument,
    source: OpenApiSource,
    models: BTreeMap<String, Model>,
    generated_model_names: BTreeSet<String>,
    generated_operation_names: BTreeSet<String>,
    local_ref_model_names: BTreeMap<String, String>,
    active_model_builds: BTreeSet<String>,
    active_local_ref_imports: BTreeSet<String>,
    normalized_all_of_refs: BTreeMap<String, Schema>,
    active_all_of_refs: Vec<String>,
    active_object_view_refs: Vec<String>,
    warnings: Vec<OpenApiDiagnostic>,
    options: LoadOpenApiOptions,
}

impl OpenApiImporter {
    fn new(document: OpenApiDocument, source: OpenApiSource, options: LoadOpenApiOptions) -> Self {
        Self {
            document,
            source,
            models: BTreeMap::new(),
            generated_model_names: BTreeSet::new(),
            generated_operation_names: BTreeSet::new(),
            local_ref_model_names: BTreeMap::new(),
            active_model_builds: BTreeSet::new(),
            active_local_ref_imports: BTreeSet::new(),
            normalized_all_of_refs: BTreeMap::new(),
            active_all_of_refs: Vec::new(),
            active_object_view_refs: Vec::new(),
            warnings: Vec::new(),
            options,
        }
    }

    fn build_ir(mut self) -> Result<OpenApiLoadResult> {
        measure_openapi_phase(
            self.options.emit_timings,
            "openapi_component_models",
            || self.import_component_models(),
        )?;

        let mut operations = Vec::new();
        measure_openapi_phase(self.options.emit_timings, "openapi_operations", || {
            let paths = self.document.paths.clone();
            for (path, item) in &paths {
                operations.extend(self.import_path_item(path, item)?);
            }
            Ok(())
        })?;

        let ir = CoreIr {
            models: self.models.into_values().collect(),
            operations,
            ..Default::default()
        };

        measure_openapi_phase(self.options.emit_timings, "openapi_validate_ir", || {
            validate_ir(&ir).map_err(|errors| {
                let details = errors
                    .0
                    .iter()
                    .map(|issue| format!("{}: {}", issue.path, issue.message))
                    .collect::<Vec<_>>()
                    .join("\n");
                anyhow!("generated IR is invalid:\n{details}")
            })
        })?;
        Ok(OpenApiLoadResult {
            ir,
            warnings: self.warnings,
        })
    }

    fn import_component_models(&mut self) -> Result<()> {
        let mut schemas = Vec::new();
        for (name, schema) in self.document.components.schemas.clone() {
            let pointer = format!("#/components/schemas/{name}");
            schemas.push((name, schema, pointer));
        }
        for (name, schema) in self.document.definitions.clone() {
            let pointer = format!("#/definitions/{name}");
            schemas.push((name, schema, pointer));
        }
        let total = schemas.len();
        for (index, (name, schema, pointer)) in schemas.into_iter().enumerate() {
            if self.options.emit_timings {
                eprintln!(
                    "timing: starting component_model [{}/{}] {}",
                    index + 1,
                    total,
                    name
                );
            }
            let started = Instant::now();
            self.ensure_named_schema_model(&name, &schema, &pointer)?;
            if self.options.emit_timings {
                eprintln!(
                    "timing: component_model [{}/{}] {:<40} {}",
                    index + 1,
                    total,
                    name,
                    format_duration(started.elapsed())
                );
            }
        }
        Ok(())
    }

    fn import_path_item(&mut self, path: &str, item: &PathItem) -> Result<Vec<Operation>> {
        let mut operations = Vec::new();
        let shared_parameters = item.parameters.clone().unwrap_or_default();
        let candidates = [
            (HttpMethod::Get, item.get.as_ref()),
            (HttpMethod::Post, item.post.as_ref()),
            (HttpMethod::Put, item.put.as_ref()),
            (HttpMethod::Patch, item.patch.as_ref()),
            (HttpMethod::Delete, item.delete.as_ref()),
        ];

        for (method, spec) in candidates {
            let Some(spec) = spec else {
                continue;
            };

            let operation_name = self.reserve_operation_name(
                spec.operation_id
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned)
                    .unwrap_or_else(|| fallback_operation_name(method, path)),
            );
            let mut operation = Operation {
                id: format!("operation.{operation_name}"),
                name: operation_name.clone(),
                method,
                path: path.to_owned(),
                params: Vec::new(),
                request_body: None,
                responses: Vec::new(),
                attributes: operation_attributes(spec),
                source: Some(SourceRef {
                    pointer: format!("#/paths/{}/{}", json_pointer_key(path), method_key(method)),
                    line: None,
                }),
            };
            let mut unnamed_parameter_counter = 0usize;
            let mut form_data_parameters = Vec::new();
            let shared_len = shared_parameters.len();

            for (param_idx, param) in shared_parameters.iter().chain(spec.parameters.iter()).enumerate() {
                let mut resolved = self.resolve_parameter(param)?;
                if resolved.name.trim().is_empty() {
                    unnamed_parameter_counter += 1;
                    // Use the specific parameter pointer so the source preview
                    // and line number point at the offending parameter item
                    // rather than the whole operation.
                    let param_pointer = if param_idx < shared_len {
                        format!("#/paths/{}/parameters/{}", json_pointer_key(path), param_idx)
                    } else {
                        format!(
                            "#/paths/{}/{}/parameters/{}",
                            json_pointer_key(path),
                            method_key(method),
                            param_idx - shared_len,
                        )
                    };
                    self.handle_unhandled(
                        &param_pointer,
                        DiagnosticKind::EmptyParameterName {
                            counter: unnamed_parameter_counter,
                        },
                    )?;
                    resolved.name = format!(
                        "unnamed_{}_parameter_{}",
                        raw_parameter_location_label(resolved.location),
                        unnamed_parameter_counter
                    );
                }
                if resolved.location == RawParameterLocation::Body {
                    let request_body =
                        self.import_swagger_body_parameter(&resolved, spec, &operation_name)?;
                    if operation.request_body.is_some() {
                        bail!(self.make_diagnostic(
                            &format!("operation `{operation_name}`"),
                            DiagnosticKind::MultipleRequestBodyDeclarations {
                                note: "Arvalez can normalize either an OpenAPI `requestBody` or a single Swagger 2 `in: body` parameter for an operation.".into(),
                            },
                        ));
                    }
                    operation.request_body = Some(request_body);
                    continue;
                }
                if resolved.location == RawParameterLocation::FormData {
                    form_data_parameters.push(resolved);
                    continue;
                }

                operation.params.push(self.import_parameter(&resolved)?);
            }

            if !form_data_parameters.is_empty() {
                if operation.request_body.is_some() {
                    bail!(self.make_diagnostic(
                        &format!("operation `{operation_name}`"),
                        DiagnosticKind::MultipleRequestBodyDeclarations {
                            note: "Arvalez can normalize either an OpenAPI `requestBody`, a single Swagger 2 `in: body` parameter, or Swagger 2 `formData` parameters for an operation.".into(),
                        },
                    ));
                }
                operation.request_body = Some(self.import_swagger_form_data_request_body(
                    &form_data_parameters,
                    spec,
                    &operation_name,
                )?);
            }

            if let Some(request_body) = &spec.request_body {
                if operation.request_body.is_some() {
                    bail!(self.make_diagnostic(
                        &format!("operation `{operation_name}`"),
                        DiagnosticKind::MultipleRequestBodyDeclarations {
                            note: "Arvalez can normalize either an OpenAPI `requestBody` or a single Swagger 2 `in: body` parameter for an operation.".into(),
                        },
                    ));
                }
                operation.request_body =
                    Some(self.import_request_body(request_body, &operation_name, path, method)?);
            }

            for (status, response_or_ref) in &spec.responses {
                let response = self.resolve_response_spec(response_or_ref)?;
                operation.responses.push(self.import_response(
                    status,
                    &response,
                    &operation_name,
                    path,
                    method,
                )?);
            }

            operations.push(operation);
        }

        Ok(operations)
    }

    fn import_parameter(&mut self, param: &ParameterSpec) -> Result<Parameter> {
        let schema = param.effective_schema().ok_or_else(|| {
            anyhow::Error::new(self.make_diagnostic(
                &format!("parameter `{}`", param.name),
                DiagnosticKind::ParameterMissingSchema {
                    name: param.name.clone(),
                },
            ))
        })?;
        let imported = self.import_schema_type(
            &schema,
            &InlineModelContext::Parameter {
                name: param.name.clone(),
            },
        )?;

        Ok(Parameter {
            name: param.name.clone(),
            location: param.location.as_ir_location().ok_or_else(|| {
                anyhow::Error::new(self.make_diagnostic(
                    &format!("parameter `{}`", param.name),
                    DiagnosticKind::UnsupportedParameterLocation {
                        name: param.name.clone(),
                    },
                ))
            })?,
            type_ref: imported
                .type_ref
                .unwrap_or_else(|| TypeRef::primitive("any")),
            required: param.required,
            attributes: parameter_attributes(&param, &schema),
        })
    }

    fn import_swagger_body_parameter(
        &mut self,
        param: &ParameterSpec,
        spec: &OperationSpec,
        operation_name: &str,
    ) -> Result<RequestBody> {
        let schema = param.effective_schema().ok_or_else(|| {
            anyhow::Error::new(self.make_diagnostic(
                &format!("body parameter `{}`", param.name),
                DiagnosticKind::BodyParameterMissingSchema {
                    name: param.name.clone(),
                },
            ))
        })?;

        let imported = self.import_schema_type(
            &schema,
            &InlineModelContext::RequestBody {
                operation_name: operation_name.to_owned(),
                pointer: format!(
                    "#/operations/{operation_name}/body_parameter/{}",
                    param.name
                ),
            },
        )?;

        let media_type = spec
            .consumes
            .first()
            .cloned()
            .or_else(|| self.document.consumes.first().cloned())
            .unwrap_or_else(|| "application/json".into());

        let mut attributes = schema_runtime_attributes(&schema);
        if !param.description.trim().is_empty() {
            attributes.insert(
                "description".into(),
                Value::String(param.description.trim().to_owned()),
            );
        }

        Ok(RequestBody {
            required: param.required,
            media_type,
            type_ref: imported.type_ref,
            attributes,
        })
    }

    fn import_swagger_form_data_request_body(
        &mut self,
        params: &[ParameterSpec],
        spec: &OperationSpec,
        operation_name: &str,
    ) -> Result<RequestBody> {
        let mut properties = IndexMap::new();
        let mut required = Vec::new();
        for param in params {
            let mut schema = param.effective_schema().ok_or_else(|| {
                anyhow::Error::new(self.make_diagnostic(
                    &format!("formData parameter `{}`", param.name),
                    DiagnosticKind::FormDataParameterMissingSchema {
                        name: param.name.clone(),
                    },
                ))
            })?;
            if !param.description.trim().is_empty() {
                schema.extra_keywords.insert(
                    "description".into(),
                    Value::String(param.description.trim().to_owned()),
                );
            }
            if param.required {
                required.push(param.name.clone());
            }
            properties.insert(param.name.clone(), SchemaOrBool::Schema(schema));
        }

        let imported = self.import_schema_type(
            &Schema {
                schema_type: Some(SchemaTypeDecl::Single("object".into())),
                properties: Some(properties),
                required: (!required.is_empty()).then_some(required.clone()),
                ..Schema::default()
            },
            &InlineModelContext::RequestBody {
                operation_name: operation_name.to_owned(),
                pointer: format!("#/operations/{operation_name}/formData"),
            },
        )?;

        let media_type = spec
            .consumes
            .first()
            .cloned()
            .or_else(|| self.document.consumes.first().cloned())
            .unwrap_or_else(|| "application/x-www-form-urlencoded".into());

        let mut attributes = Attributes::default();
        if params.iter().any(|param| param.required) {
            attributes.insert("form_encoding".into(), Value::String(media_type.clone()));
        }

        Ok(RequestBody {
            required: params.iter().any(|param| param.required),
            media_type,
            type_ref: imported.type_ref,
            attributes,
        })
    }

    fn resolve_parameter(&self, param: &ParameterOrRef) -> Result<ParameterSpec> {
        let mut seen = BTreeSet::new();
        self.resolve_parameter_inner(param, &mut seen)
    }

    fn resolve_parameter_inner(
        &self,
        param: &ParameterOrRef,
        seen: &mut BTreeSet<String>,
    ) -> Result<ParameterSpec> {
        match param {
            ParameterOrRef::Inline(param) => Ok(param.clone()),
            ParameterOrRef::Ref { reference } => {
                if !seen.insert(reference.clone()) {
                    bail!(self.make_pointer_diagnostic(
                        reference,
                        DiagnosticKind::RecursiveParameterCycle {
                            reference: reference.to_owned()
                        },
                    ));
                }

                if let Some(parameter) = self.resolve_named_parameter_reference(reference) {
                    return self
                        .resolve_parameter_inner(&ParameterOrRef::Inline(parameter.clone()), seen);
                }

                if let Some(parameter) = self.resolve_path_parameter_reference(reference)? {
                    return self.resolve_parameter_inner(parameter, seen);
                }

                Err(anyhow!("unsupported reference `{reference}`"))
            }
        }
    }

    fn resolve_named_parameter_reference(&self, reference: &str) -> Option<&ParameterSpec> {
        let name = ref_name(reference).ok()?;
        self.document
            .components
            .parameters
            .get(&name)
            .or_else(|| self.document.parameters.get(&name))
    }

    fn resolve_path_parameter_reference<'a>(
        &'a self,
        reference: &str,
    ) -> Result<Option<&'a ParameterOrRef>> {
        let Some(pointer) = reference.strip_prefix("#/") else {
            return Ok(None);
        };
        let segments = pointer
            .split('/')
            .map(decode_json_pointer_segment)
            .collect::<Result<Vec<_>>>()?;
        if segments.first().map(String::as_str) != Some("paths") {
            return Ok(None);
        }

        match segments.as_slice() {
            [_, path, scope, index] if scope == "parameters" => {
                let index = index.parse::<usize>().ok();
                let param = self
                    .document
                    .paths
                    .get(path)
                    .and_then(|item| item.parameters.as_ref())
                    .and_then(|params| index.and_then(|idx| params.get(idx)));
                Ok(param)
            }
            [_, path, method, scope, index] if scope == "parameters" => {
                let index = index.parse::<usize>().ok();
                let param = self
                    .document
                    .paths
                    .get(path)
                    .and_then(|item| match method.as_str() {
                        "get" => item.get.as_ref(),
                        "post" => item.post.as_ref(),
                        "put" => item.put.as_ref(),
                        "patch" => item.patch.as_ref(),
                        "delete" => item.delete.as_ref(),
                        _ => None,
                    })
                    .and_then(|operation| index.and_then(|idx| operation.parameters.get(idx)));
                Ok(param)
            }
            _ => Ok(None),
        }
    }

    fn import_request_body(
        &mut self,
        request_body: &RequestBodyOrRef,
        operation_name: &str,
        path: &str,
        method: HttpMethod,
    ) -> Result<RequestBody> {
        let fallback_pointer = format!(
            "#/paths/{}/{}/requestBody",
            json_pointer_key(path),
            method_key(method)
        );
        let (request_body, pointer) = self.resolve_request_body(request_body, &fallback_pointer)?;
        let content_pointer = format!("{pointer}/content");
        let Some((media_type, media_spec)) = request_body.content.iter().next() else {
            self.warnings.push(self.make_pointer_diagnostic(
                &content_pointer,
                DiagnosticKind::EmptyRequestBodyContent,
            ));
            return Ok(RequestBody {
                required: request_body.required,
                media_type: "application/octet-stream".into(),
                type_ref: None,
                attributes: Attributes::default(),
            });
        };

        let imported = media_spec
            .schema
            .as_ref()
            .map(|schema| {
                self.import_schema_type(
                    schema,
                    &InlineModelContext::RequestBody {
                        operation_name: operation_name.to_owned(),
                        pointer: format!(
                            "{content_pointer}/{}/schema",
                            json_pointer_key(media_type)
                        ),
                    },
                )
            })
            .transpose()?;

        Ok(RequestBody {
            required: request_body.required,
            media_type: media_type.clone(),
            type_ref: imported.and_then(|value| value.type_ref),
            attributes: media_spec
                .schema
                .as_ref()
                .map(schema_runtime_attributes)
                .unwrap_or_default(),
        })
    }

    fn resolve_request_body(
        &self,
        request_body: &RequestBodyOrRef,
        pointer: &str,
    ) -> Result<(RequestBodySpec, String)> {
        let mut seen = BTreeSet::new();
        self.resolve_request_body_inner(request_body, pointer, &mut seen)
    }

    fn resolve_request_body_inner(
        &self,
        request_body: &RequestBodyOrRef,
        pointer: &str,
        seen: &mut BTreeSet<String>,
    ) -> Result<(RequestBodySpec, String)> {
        match request_body {
            RequestBodyOrRef::Inline(spec) => Ok((spec.clone(), pointer.to_owned())),
            RequestBodyOrRef::Ref { reference } => {
                if !seen.insert(reference.clone()) {
                    bail!(self.make_pointer_diagnostic(
                        reference,
                        DiagnosticKind::RecursiveRequestBodyCycle {
                            reference: reference.to_owned()
                        },
                    ));
                }
                let name = ref_name(reference)?;
                let referenced = self
                    .document
                    .components
                    .request_bodies
                    .get(&name)
                    .ok_or_else(|| anyhow!("unsupported reference `{reference}`"))?;
                self.resolve_request_body_inner(referenced, reference, seen)
            }
        }
    }

    fn resolve_response_spec(&self, response: &ResponseSpecOrRef) -> Result<ResponseSpec> {
        match &response.reference {
            None => Ok(ResponseSpec {
                description: response.description.clone(),
                content: response.content.clone(),
            }),
            Some(reference) => {
                let Some(pointer) = reference.strip_prefix("#/") else {
                    return Err(anyhow!("unsupported reference `{reference}`"));
                };
                let segments: Vec<&str> = pointer.split('/').collect();
                match segments.as_slice() {
                    // OpenAPI 3: #/components/responses/{name}
                    ["components", "responses", name] => self
                        .document
                        .components
                        .responses
                        .get(*name)
                        .cloned()
                        .ok_or_else(|| anyhow!("unsupported reference `{reference}`")),
                    // Swagger 2: #/responses/{name}
                    ["responses", name] => self
                        .document
                        .responses
                        .get(*name)
                        .cloned()
                        .ok_or_else(|| anyhow!("unsupported reference `{reference}`")),
                    // Inline path reference (e.g. #/paths/.../responses/200) — return
                    // empty response, preserving the same silent-fallback behaviour that
                    // existed before ResponseSpecOrRef was introduced.
                    _ => Ok(ResponseSpec::default()),
                }
            }
        }
    }

    fn import_response(
        &mut self,
        status: &str,
        response: &ResponseSpec,
        operation_name: &str,
        path: &str,
        method: HttpMethod,
    ) -> Result<Response> {
        let (media_type, schema) = response
            .content
            .iter()
            .find_map(|(media_type, media)| {
                media.schema.as_ref().map(|schema| (media_type, schema))
            })
            .map(|(media_type, schema)| (Some(media_type.clone()), Some(schema)))
            .unwrap_or((None, None));

        let imported = schema
            .map(|schema| {
                self.import_schema_type(
                    schema,
                    &InlineModelContext::Response {
                        operation_name: operation_name.to_owned(),
                        status: status.to_owned(),
                        pointer: media_type.as_ref().map_or_else(
                            || {
                                format!(
                                    "#/paths/{}/{}/responses/{}",
                                    json_pointer_key(path),
                                    method_key(method),
                                    json_pointer_key(status)
                                )
                            },
                            |media_type| {
                                format!(
                                    "#/paths/{}/{}/responses/{}/content/{}/schema",
                                    json_pointer_key(path),
                                    method_key(method),
                                    json_pointer_key(status),
                                    json_pointer_key(media_type)
                                )
                            },
                        ),
                    },
                )
            })
            .transpose()?;

        let mut attributes = Attributes::default();
        if !response.description.is_empty() {
            attributes.insert(
                "description".into(),
                Value::String(response.description.clone()),
            );
        }
        if let Some(schema) = schema {
            attributes.extend(schema_runtime_attributes(schema));
        }

        Ok(Response {
            status: status.to_owned(),
            media_type,
            type_ref: imported.and_then(|value| value.type_ref),
            attributes,
        })
    }

    fn ensure_named_schema_model(
        &mut self,
        name: &str,
        schema: &Schema,
        pointer: &str,
    ) -> Result<()> {
        if self.models.contains_key(name) {
            return Ok(());
        }

        let model = self.build_model_from_schema(name, schema, pointer)?;
        self.generated_model_names.insert(name.to_owned());
        self.models.insert(name.to_owned(), model);
        Ok(())
    }

    fn build_model_from_schema(
        &mut self,
        name: &str,
        schema: &Schema,
        pointer: &str,
    ) -> Result<Model> {
        if schema.all_of.is_some() && schema_is_object_like(schema) {
            return self.build_object_model_from_all_of(name, schema, pointer);
        }

        let schema = self.normalize_schema(schema, pointer)?;
        let schema = schema.as_ref();
        self.validate_schema_keywords(schema, pointer)?;

        if let Some(enum_values) = &schema.enum_values {
            let mut model = Model::new(format!("model.{}", to_snake_case(name)), name.to_owned());
            model.source = Some(SourceRef {
                pointer: pointer.to_owned(),
                line: None,
            });
            model
                .attributes
                .insert("enum_values".into(), Value::Array(enum_values.clone()));
            if let Some(schema_type) = schema.primary_schema_type() {
                model.attributes.insert(
                    "enum_base_type".into(),
                    Value::String(schema_type.to_owned()),
                );
            }
            if let Some(title) = &schema.title {
                model
                    .attributes
                    .insert("title".into(), Value::String(title.clone()));
            }
            return Ok(model);
        }

        if !schema_is_object_like(schema) {
            let imported = self.import_schema_type_normalized(
                schema,
                &InlineModelContext::NamedSchema {
                    name: name.to_owned(),
                    pointer: pointer.to_owned(),
                },
                true,
                None,
            )?;
            let mut model = Model::new(format!("model.{}", to_snake_case(name)), name.to_owned());
            model.source = Some(SourceRef {
                pointer: pointer.to_owned(),
                line: None,
            });
            model.attributes.insert(
                "alias_type_ref".into(),
                json!(
                    imported
                        .type_ref
                        .unwrap_or_else(|| TypeRef::primitive("any"))
                ),
            );
            model
                .attributes
                .insert("alias_nullable".into(), Value::Bool(imported.nullable));
            model.attributes.extend(schema_runtime_attributes(schema));
            if let Some(title) = &schema.title {
                model
                    .attributes
                    .insert("title".into(), Value::String(title.clone()));
            }
            return Ok(model);
        }

        let empty_properties = IndexMap::new();
        let properties = schema.properties.as_ref().unwrap_or(&empty_properties);
        let required: BTreeSet<&str> = schema
            .required
            .iter()
            .flat_map(|items| items.iter().map(String::as_str))
            .collect();

        let mut model = Model::new(format!("model.{}", to_snake_case(name)), name.to_owned());
        model.source = Some(SourceRef {
            pointer: pointer.to_owned(),
            line: None,
        });
        if let Some(title) = &schema.title {
            model
                .attributes
                .insert("title".into(), Value::String(title.clone()));
        }

        let mut unnamed_field_counter = 0usize;
        for (field_name, property_schema_or_bool) in properties {
            // Boolean schemas (OpenAPI 3.1: `false`/`true`) have no codegen meaning — skip.
            let Some(property_schema) = property_schema_or_bool.as_schema() else {
                continue;
            };
            let original_field_name = field_name.clone();
            let field_name = self.normalize_field_name(
                field_name.clone(),
                &format!("{pointer}/properties"),
                &mut unnamed_field_counter,
            )?;
            let imported = self.import_schema_type(
                property_schema,
                &InlineModelContext::Field {
                    model_name: name.to_owned(),
                    field_name: original_field_name.clone(),
                    pointer: format!(
                        "{}/properties/{}",
                        pointer,
                        json_pointer_key(&original_field_name)
                    ),
                },
            )?;
            let mut field = Field::new(
                field_name.clone(),
                imported
                    .type_ref
                    .unwrap_or_else(|| TypeRef::primitive("any")),
            );
            field.optional = !required.contains(original_field_name.as_str());
            field.nullable = imported.nullable;
            field.attributes = schema_runtime_attributes(property_schema);
            model.fields.push(field);
        }

        Ok(model)
    }

    fn build_object_model_from_all_of(
        &mut self,
        name: &str,
        schema: &Schema,
        pointer: &str,
    ) -> Result<Model> {
        let view = self.collect_object_schema_view(schema, pointer)?;
        let required = view.required;

        let mut model = Model::new(format!("model.{}", to_snake_case(name)), name.to_owned());
        model.source = Some(SourceRef {
            pointer: pointer.to_owned(),
            line: None,
        });
        if let Some(title) = view.title {
            model
                .attributes
                .insert("title".into(), Value::String(title));
        }

        let mut unnamed_field_counter = 0usize;
        for (field_name, property_schema) in view.properties {
            let original_field_name = field_name.clone();
            let field_name = self.normalize_field_name(
                field_name,
                &format!("{pointer}/properties"),
                &mut unnamed_field_counter,
            )?;
            let imported = self.import_schema_type(
                &property_schema,
                &InlineModelContext::Field {
                    model_name: name.to_owned(),
                    field_name: original_field_name.clone(),
                    pointer: format!(
                        "{}/properties/{}",
                        pointer,
                        json_pointer_key(&original_field_name)
                    ),
                },
            )?;
            let mut field = Field::new(
                field_name.clone(),
                imported
                    .type_ref
                    .unwrap_or_else(|| TypeRef::primitive("any")),
            );
            field.optional = !required.contains(original_field_name.as_str());
            field.nullable = imported.nullable;
            field.attributes = schema_runtime_attributes(&property_schema);
            model.fields.push(field);
        }

        Ok(model)
    }

    fn import_schema_type(
        &mut self,
        schema: &Schema,
        context: &InlineModelContext,
    ) -> Result<ImportedType> {
        self.import_schema_type_inner(schema, context, false)
    }

    fn import_schema_type_inner(
        &mut self,
        schema: &Schema,
        context: &InlineModelContext,
        skip_keyword_validation: bool,
    ) -> Result<ImportedType> {
        let local_reference = schema
            .reference
            .as_deref()
            .filter(|reference| is_inline_local_schema_reference(reference))
            .map(ToOwned::to_owned);
        if let Some(imported) = self.import_decorated_reference_type(schema, context)? {
            return Ok(imported);
        }
        let schema = self.normalize_schema(schema, &context.describe())?;
        self.import_schema_type_normalized(
            schema.as_ref(),
            context,
            skip_keyword_validation,
            local_reference.as_deref(),
        )
    }

    fn import_decorated_reference_type(
        &mut self,
        schema: &Schema,
        context: &InlineModelContext,
    ) -> Result<Option<ImportedType>> {
        if matches!(context, InlineModelContext::NamedSchema { .. }) {
            return Ok(None);
        }

        let all_of = match &schema.all_of {
            Some(all_of) => all_of,
            None => return Ok(None),
        };

        if schema_has_non_all_of_shape(schema) {
            return Ok(None);
        }

        let mut reference: Option<&str> = None;
        for member in all_of {
            if let Some(member_ref) = member.reference.as_deref() {
                if reference.replace(member_ref).is_some() {
                    return Ok(None);
                }
                continue;
            }

            if !is_unconstrained_schema(member) {
                return Ok(None);
            }
        }

        let Some(reference) = reference else {
            return Ok(None);
        };

        Ok(Some(ImportedType {
            type_ref: Some(TypeRef::named(ref_name(reference)?)),
            nullable: false,
        }))
    }

    fn import_schema_type_normalized(
        &mut self,
        schema: &Schema,
        context: &InlineModelContext,
        skip_keyword_validation: bool,
        local_reference: Option<&str>,
    ) -> Result<ImportedType> {
        if !skip_keyword_validation {
            self.validate_schema_keywords(schema, &context.describe())?;
        }

        if let Some(reference) = &schema.reference {
            if is_inline_local_schema_reference(reference) {
                if self.active_local_ref_imports.contains(reference) {
                    let model_name = self
                        .local_ref_model_names
                        .get(reference)
                        .cloned()
                        .unwrap_or_else(|| {
                            to_pascal_case(
                                &ref_name(reference).unwrap_or_else(|_| "RecursiveModel".into()),
                            )
                        });
                    return Ok(ImportedType::plain(TypeRef::named(model_name)));
                }

                self.active_local_ref_imports.insert(reference.clone());
                // Use the cycle-safe resolve+expand path so that allOf schemas
                // (e.g. `{allOf: [{$ref: "..."}]}`) are fully shaped before
                // import_schema_type_normalized sees them, without risking
                // infinite recursion on self-referential schemas.
                let resolved =
                    self.resolve_schema_reference_for_all_of(reference, &context.describe())?;
                if schema_is_object_like(&resolved) {
                    let already_registered = self.local_ref_model_names.contains_key(reference);
                    if !already_registered {
                        let model_name = self.inline_model_name(&resolved, context);
                        self.local_ref_model_names
                            .insert(reference.clone(), model_name);
                    } else {
                        // Model was already imported from a previous (non-recursive) call site.
                        // Return a named reference immediately to avoid re-processing the full
                        // expanded schema from each call site, which would create exponentially
                        // many inline models for mutually-referential schemas (e.g. Azure specs).
                        let model_name = self.local_ref_model_names[reference].clone();
                        self.active_local_ref_imports.remove(reference);
                        return Ok(ImportedType::plain(TypeRef::named(model_name)));
                    }
                }
                let result = self.import_schema_type_normalized(
                    &resolved,
                    context,
                    skip_keyword_validation,
                    Some(reference),
                );
                self.active_local_ref_imports.remove(reference);
                return result;
            }
            return Ok(ImportedType {
                type_ref: Some(TypeRef::named(ref_name(reference)?)),
                nullable: false,
            });
        }

        if let Some(const_value) = &schema.const_value {
            return self.import_const_type(&schema, const_value, context);
        }

        if schema_is_object_like(schema)
            && schema
                .any_of
                .as_ref()
                .is_some_and(|variants| variants.iter().all(is_validation_only_schema_variant))
        {
            return self.import_object_type(schema, context, local_reference);
        }

        if let Some(any_of) = &schema.any_of {
            return self.import_any_of(any_of, context);
        }

        if schema_is_object_like(schema)
            && schema
                .one_of
                .as_ref()
                .is_some_and(|variants| variants.iter().all(is_validation_only_schema_variant))
        {
            return self.import_object_type(schema, context, local_reference);
        }

        if let Some(one_of) = &schema.one_of {
            return self.import_any_of(one_of, context);
        }

        if let Some(imported) = self.import_implicit_schema_type(schema, context)? {
            return Ok(imported);
        }

        if let Some(imported) = self.import_schema_type_from_decl(&schema, context)? {
            return Ok(imported);
        }

        if is_unconstrained_schema(&schema) {
            return Ok(ImportedType::plain(TypeRef::primitive("any")));
        }

        if schema.properties.is_some() || schema.additional_properties.is_some() {
            return self.import_object_type(&schema, context, local_reference);
        }

        self.handle_unhandled(&context.describe(), DiagnosticKind::UnsupportedSchemaShape)?;
        Ok(ImportedType::plain(TypeRef::primitive("any")))
    }

    fn import_schema_type_from_decl(
        &mut self,
        schema: &Schema,
        context: &InlineModelContext,
    ) -> Result<Option<ImportedType>> {
        let Some(schema_types) = &schema.schema_type else {
            return Ok(None);
        };

        if let Some(embedded) = schema_types.embedded_schema() {
            return Ok(Some(self.import_schema_type(embedded, context)?));
        }

        let variants = schema_types.as_slice();
        if variants.len() == 1 {
            let schema_type = variants[0].as_str();
            return Ok(Some(match schema_type {
                "string" => {
                    if schema.format.as_deref() == Some("binary") {
                        ImportedType::plain(TypeRef::primitive("binary"))
                    } else {
                        ImportedType::plain(TypeRef::primitive("string"))
                    }
                }
                "integer" => ImportedType::plain(TypeRef::primitive("integer")),
                "number" => ImportedType::plain(TypeRef::primitive("number")),
                "boolean" => ImportedType::plain(TypeRef::primitive("boolean")),
                "array" => {
                    match schema.items.as_ref() {
                        Some(item_schema) => {
                            let imported = self.import_schema_type(item_schema, context)?;
                            ImportedType::plain(TypeRef::array(
                                imported
                                    .type_ref
                                    .unwrap_or_else(|| TypeRef::primitive("any")),
                            ))
                        }
                        // JSON Schema: array without `items` means array of any.
                        None => ImportedType::plain(TypeRef::array(TypeRef::primitive("any"))),
                    }
                }
                "object" => self.import_object_type(schema, context, None)?,
                "file" => ImportedType::plain(TypeRef::primitive("binary")),
                "null" => ImportedType {
                    type_ref: Some(TypeRef::primitive("any")),
                    nullable: true,
                },
                other => {
                    self.handle_unhandled(
                        &context.describe(),
                        DiagnosticKind::UnsupportedSchemaType {
                            schema_type: other.to_owned(),
                        },
                    )?;
                    ImportedType::plain(TypeRef::primitive("any"))
                }
            }));
        }

        let mut nullable = false;
        let mut type_refs = Vec::new();
        for schema_type in variants {
            match schema_type.as_str() {
                "null" => nullable = true,
                other => {
                    let mut synthetic = schema.clone();
                    synthetic.schema_type = Some(SchemaTypeDecl::Single(other.to_owned()));
                    let imported = self
                        .import_schema_type_from_decl(&synthetic, context)?
                        .expect("single schema type should import");
                    if imported.nullable {
                        nullable = true;
                    }
                    if let Some(type_ref) = imported.type_ref {
                        type_refs.push(type_ref);
                    }
                }
            }
        }

        let type_refs = dedupe_variants(type_refs);
        let type_ref = match type_refs.len() {
            0 => Some(TypeRef::primitive("any")),
            1 => type_refs.into_iter().next(),
            _ => Some(TypeRef::Union {
                variants: type_refs,
            }),
        };

        Ok(Some(ImportedType { type_ref, nullable }))
    }

    fn import_implicit_schema_type(
        &mut self,
        schema: &Schema,
        context: &InlineModelContext,
    ) -> Result<Option<ImportedType>> {
        if let Some(enum_values) = &schema.enum_values {
            let inferred = infer_enum_type(enum_values, schema.format.as_deref());
            return Ok(Some(ImportedType {
                type_ref: Some(inferred),
                nullable: false,
            }));
        }

        if schema.items.is_some() {
            let item_schema = schema.items.as_ref().expect("checked is_some");
            let imported = self.import_schema_type(item_schema, context)?;
            return Ok(Some(ImportedType::plain(TypeRef::array(
                imported
                    .type_ref
                    .unwrap_or_else(|| TypeRef::primitive("any")),
            ))));
        }

        if let Some(type_ref) = infer_format_only_type(schema.format.as_deref()) {
            return Ok(Some(ImportedType::plain(type_ref)));
        }

        // Format present but unrecognized by type inference (e.g. a human-readable
        // sentence used as the format value). Treat the schema as unconstrained
        // rather than failing with an unsupported-shape error.
        if schema.format.is_some() {
            return Ok(Some(ImportedType::plain(TypeRef::primitive("any"))));
        }

        Ok(None)
    }

    fn validate_schema_keywords(&mut self, schema: &Schema, context: &str) -> Result<()> {
        for keyword in schema.extra_keywords.keys() {
            if is_known_ignored_schema_keyword(keyword) || keyword.starts_with("x-") {
                continue;
            }

            if is_known_but_unimplemented_schema_keyword(keyword) {
                self.handle_unhandled(
                    context,
                    DiagnosticKind::UnsupportedSchemaKeyword {
                        keyword: keyword.clone(),
                    },
                )?;
                continue;
            }

            self.handle_unhandled(
                context,
                DiagnosticKind::UnknownSchemaKeyword {
                    keyword: keyword.clone(),
                },
            )?;
        }

        Ok(())
    }

    fn normalize_schema<'a>(
        &mut self,
        schema: &'a Schema,
        context: &str,
    ) -> Result<Cow<'a, Schema>> {
        if schema.all_of.is_none() {
            return Ok(Cow::Borrowed(schema));
        }

        let normalized = self.expand_all_of_schema(schema, context)?;
        Ok(Cow::Owned(normalized))
    }

    fn expand_all_of_schema(&mut self, schema: &Schema, context: &str) -> Result<Schema> {
        let mut merged = Schema {
            all_of: None,
            ..schema.clone()
        };

        for member in schema.all_of.clone().unwrap_or_default() {
            let resolved_member = self.resolve_schema_for_merge(&member, context)?;
            merged = self.merge_schemas(merged, resolved_member, context)?;
        }

        Ok(merged)
    }

    fn resolve_schema_for_merge(&mut self, schema: &Schema, context: &str) -> Result<Schema> {
        let mut resolved = if let Some(reference) = &schema.reference {
            self.resolve_schema_reference_for_all_of(reference, context)?
        } else {
            schema.clone()
        };

        if resolved.all_of.is_some() {
            resolved = self.expand_all_of_schema(&resolved, context)?;
        }

        if schema.reference.is_some() {
            let mut overlay = schema.clone();
            overlay.reference = None;
            overlay.all_of = None;
            resolved = self.merge_schemas(resolved, overlay, context)?;
        }

        Ok(resolved)
    }

    fn resolve_schema_reference_for_all_of(
        &mut self,
        reference: &str,
        context: &str,
    ) -> Result<Schema> {
        if let Some(cached) = self.normalized_all_of_refs.get(reference) {
            return Ok(cached.clone());
        }

        if self.active_all_of_refs.iter().any(|item| item == reference) {
            self.handle_unhandled(
                context,
                DiagnosticKind::AllOfRecursiveCycle {
                    reference: reference.to_owned(),
                },
            )?;
            return Ok(Schema::default());
        }

        self.active_all_of_refs.push(reference.to_owned());
        let result: Result<Schema> = (|| {
            let mut resolved = self.resolve_schema_reference(reference)?;
            if resolved.all_of.is_some() {
                resolved = self.expand_all_of_schema(&resolved, reference)?;
            }
            Ok(resolved)
        })();
        self.active_all_of_refs.pop();

        let resolved = result?;
        self.normalized_all_of_refs
            .insert(reference.to_owned(), resolved.clone());
        Ok(resolved)
    }

    fn resolve_schema_reference(&self, reference: &str) -> Result<Schema> {
        let Some(pointer) = reference.strip_prefix("#/") else {
            bail!("unsupported reference `{reference}`");
        };
        let segments = pointer
            .split('/')
            .map(decode_json_pointer_segment)
            .collect::<Result<Vec<_>>>()?;
        enum ResolvedSchemaRef<'a> {
            Borrowed(&'a Schema),
            Owned(Schema),
        }

        let (resolved, remainder): (ResolvedSchemaRef<'_>, &[String]) = match segments.as_slice() {
            [root, collection, name, rest @ ..]
                if root == "components" && collection == "schemas" =>
            {
                (
                    ResolvedSchemaRef::Borrowed(
                        self.document
                            .components
                            .schemas
                            .get(name)
                            .ok_or_else(|| anyhow!("unsupported reference `{reference}`"))?,
                    ),
                    rest,
                )
            }
            [root, name, rest @ ..] if root == "definitions" => (
                ResolvedSchemaRef::Borrowed(
                    self.document
                        .definitions
                        .get(name)
                        .ok_or_else(|| anyhow!("unsupported reference `{reference}`"))?,
                ),
                rest,
            ),
            [root, collection, name, schema_segment, rest @ ..]
                if root == "components"
                    && collection == "parameters"
                    && schema_segment == "schema" =>
            {
                (
                    ResolvedSchemaRef::Owned(
                        self.document
                            .components
                            .parameters
                            .get(name)
                            .and_then(ParameterSpec::effective_schema)
                            .ok_or_else(|| anyhow!("unsupported reference `{reference}`"))?,
                    ),
                    rest,
                )
            }
            [root, name, schema_segment, rest @ ..]
                if root == "parameters" && schema_segment == "schema" =>
            {
                (
                    ResolvedSchemaRef::Owned(
                        self.document
                            .parameters
                            .get(name)
                            .and_then(ParameterSpec::effective_schema)
                            .ok_or_else(|| anyhow!("unsupported reference `{reference}`"))?,
                    ),
                    rest,
                )
            }
            // #/components/responses/{name} — use the first available schema
            [root, collection, name, rest @ ..]
                if root == "components" && collection == "responses" =>
            {
                let response = self
                    .document
                    .components
                    .responses
                    .get(name)
                    .ok_or_else(|| anyhow!("unsupported reference `{reference}`"))?;
                // `rest` may continue into content/{media_type}/schema/...
                // Resolve via the helper that understands response continuation.
                return resolve_response_schema_reference(response, rest, reference);
            }
            // #/paths/{path}/{method}/responses/{status}/content/{media}/schema
            // #/paths/{path}/{method}/responses/{status}
            [root, path, method, responses_key, status, rest @ ..]
                if root == "paths" && responses_key == "responses" =>
            {
                let operation = self
                    .document
                    .paths
                    .get(path)
                    .and_then(|item| match method.as_str() {
                        "get" => item.get.as_ref(),
                        "post" => item.post.as_ref(),
                        "put" => item.put.as_ref(),
                        "patch" => item.patch.as_ref(),
                        "delete" => item.delete.as_ref(),
                        _ => None,
                    })
                    .ok_or_else(|| anyhow!("unsupported reference `{reference}`"))?;
                let response_or_ref = operation
                    .responses
                    .get(status)
                    .ok_or_else(|| anyhow!("unsupported reference `{reference}`"))?;
                let response = self.resolve_response_spec(response_or_ref)?;
                return resolve_response_schema_reference(&response, rest, reference);
            }
            // #/paths/{path}/{method}/requestBody/content/{media_type}/schema/...
            [
                root,
                path,
                method,
                rb_key,
                content_key,
                media_type,
                schema_key,
                rest @ ..,
            ] if root == "paths"
                && rb_key == "requestBody"
                && content_key == "content"
                && schema_key == "schema" =>
            {
                let operation = self
                    .document
                    .paths
                    .get(path)
                    .and_then(|item| match method.as_str() {
                        "get" => item.get.as_ref(),
                        "post" => item.post.as_ref(),
                        "put" => item.put.as_ref(),
                        "patch" => item.patch.as_ref(),
                        "delete" => item.delete.as_ref(),
                        _ => None,
                    })
                    .ok_or_else(|| anyhow!("unsupported reference `{reference}`"))?;
                let request_body = match operation.request_body.as_ref() {
                    Some(RequestBodyOrRef::Inline(rb)) => rb,
                    _ => bail!("unsupported reference `{reference}`"),
                };
                let schema = request_body
                    .content
                    .get(media_type.as_str())
                    .and_then(|m| m.schema.as_ref())
                    .ok_or_else(|| anyhow!("unsupported reference `{reference}`"))?;
                return resolve_nested_schema_reference(schema, rest, reference);
            }
            // #/paths/{path}/{method}/parameters/{index}/schema/...
            [
                root,
                path,
                method,
                params_key,
                index_str,
                schema_key,
                rest @ ..,
            ] if root == "paths" && params_key == "parameters" && schema_key == "schema" => {
                let idx: usize = index_str
                    .parse()
                    .map_err(|_| anyhow!("unsupported reference `{reference}`"))?;
                let path_item = self
                    .document
                    .paths
                    .get(path)
                    .ok_or_else(|| anyhow!("unsupported reference `{reference}`"))?;
                let operation = match method.as_str() {
                    "get" => path_item.get.as_ref(),
                    "post" => path_item.post.as_ref(),
                    "put" => path_item.put.as_ref(),
                    "patch" => path_item.patch.as_ref(),
                    "delete" => path_item.delete.as_ref(),
                    _ => None,
                }
                .ok_or_else(|| anyhow!("unsupported reference `{reference}`"))?;
                let param_spec = operation
                    .parameters
                    .get(idx)
                    .or_else(|| {
                        path_item
                            .parameters
                            .as_ref()
                            .and_then(|params| params.get(idx))
                    })
                    .and_then(|p| match p {
                        ParameterOrRef::Inline(spec) => Some(spec),
                        _ => None,
                    })
                    .ok_or_else(|| anyhow!("unsupported reference `{reference}`"))?;
                let schema = param_spec
                    .effective_schema()
                    .ok_or_else(|| anyhow!("unsupported reference `{reference}`"))?;
                return resolve_nested_schema_reference(&schema, rest, reference);
            }
            _ => bail!("unsupported reference `{reference}`"),
        };

        let schema = match &resolved {
            ResolvedSchemaRef::Borrowed(schema) => *schema,
            ResolvedSchemaRef::Owned(schema) => schema,
        };
        resolve_nested_schema_reference(schema, remainder, reference)
    }

    fn reserve_operation_name(&mut self, base: String) -> String {
        if self.generated_operation_names.insert(base.clone()) {
            return base;
        }

        let mut counter = 2usize;
        loop {
            let candidate = format!("{base}_{counter}");
            if self.generated_operation_names.insert(candidate.clone()) {
                return candidate;
            }
            counter += 1;
        }
    }

    fn normalize_field_name(
        &mut self,
        field_name: String,
        context: &str,
        unnamed_field_counter: &mut usize,
    ) -> Result<String> {
        if !field_name.trim().is_empty() {
            return Ok(field_name);
        }

        *unnamed_field_counter += 1;
        // Point at the specific empty-string key within the properties mapping so
        // the source preview and line number resolve to the offending `"":` entry
        // rather than the `properties:` block as a whole.
        let specific_pointer = format!("{}/{}", context, json_pointer_key(&field_name));
        self.handle_unhandled(
            &specific_pointer,
            DiagnosticKind::EmptyPropertyKey {
                counter: *unnamed_field_counter,
            },
        )?;
        Ok(format!("unnamed_field_{}", unnamed_field_counter))
    }

    fn merge_schemas(
        &mut self,
        mut base: Schema,
        overlay: Schema,
        context: &str,
    ) -> Result<Schema> {
        let inferred_base_type = infer_schema_type_for_merge(&base);
        let inferred_overlay_type = infer_schema_type_for_merge(&overlay);
        let base_is_generic_object_placeholder = is_generic_object_placeholder(&base);
        let overlay_is_generic_object_placeholder = is_generic_object_placeholder(&overlay);
        let base_schema_type = base.schema_type.take();
        let overlay_schema_type = overlay.schema_type.clone();
        merge_non_codegen_optional_field(&mut base.definitions, overlay.definitions);
        merge_non_codegen_optional_field(&mut base.title, overlay.title);
        merge_non_codegen_optional_field(&mut base.format, overlay.format);
        base.schema_type = merge_schema_types(
            inferred_base_type,
            inferred_overlay_type,
            base_is_generic_object_placeholder,
            overlay_is_generic_object_placeholder,
            base_schema_type,
            overlay_schema_type,
            context,
            self,
        )?;
        merge_optional_field(
            &mut base.const_value,
            overlay.const_value,
            "const",
            context,
            self,
        )?;
        merge_non_codegen_optional_field(&mut base._discriminator, overlay._discriminator);
        base.enum_values =
            merge_enum_values(base.enum_values.take(), overlay.enum_values, context, self)?;
        // incompatible anyOf/oneOf in allOf — keep the base side rather than erroring.
        merge_non_codegen_optional_field(&mut base.any_of, overlay.any_of);
        merge_non_codegen_optional_field(&mut base.one_of, overlay.one_of);

        let base_required = base.required.take();
        let overlay_required = overlay.required;
        base.required = match (base_required, overlay_required) {
            (None, None) => None,
            (left, right) => Some(merge_required(
                left.unwrap_or_default(),
                right.unwrap_or_default(),
            )),
        };

        match (base.items.take(), overlay.items) {
            (Some(left), Some(right)) => {
                base.items = Some(Box::new(self.merge_schemas(*left, *right, context)?));
            }
            (Some(left), None) => base.items = Some(left),
            (None, Some(right)) => base.items = Some(right),
            (None, None) => {}
        }

        match (
            base.additional_properties.take(),
            overlay.additional_properties,
        ) {
            (
                Some(AdditionalProperties::Schema(left)),
                Some(AdditionalProperties::Schema(right)),
            ) => {
                base.additional_properties = Some(AdditionalProperties::Schema(Box::new(
                    self.merge_schemas(*left, *right, context)?,
                )));
            }
            (Some(AdditionalProperties::Bool(left)), Some(AdditionalProperties::Bool(right)))
                if left == right =>
            {
                base.additional_properties = Some(AdditionalProperties::Bool(left));
            }
            (Some(value), None) => base.additional_properties = Some(value),
            (None, Some(value)) => base.additional_properties = Some(value),
            (Some(left), Some(_right)) => {
                // Keep the left side; incompatible additionalProperties in allOf
                // is an under-specified schema — prefer the more descriptive branch.
                base.additional_properties = Some(left);
            }
            (None, None) => {}
        }

        let base_properties = base.properties.take();
        let overlay_properties = overlay.properties;
        base.properties = match (base_properties, overlay_properties) {
            (None, None) => None,
            (left, right) => Some(merge_properties(
                self,
                left.unwrap_or_default(),
                right.unwrap_or_default(),
                context,
            )?),
        };

        for (key, value) in overlay.extra_keywords {
            match base.extra_keywords.get(&key) {
                Some(existing) if existing != &value => {
                    if is_known_ignored_schema_keyword(&key) || key.starts_with("x-") {
                        continue;
                    }
                    self.handle_unhandled(
                        context,
                        DiagnosticKind::IncompatibleAllOfField { field: key.clone() },
                    )?;
                }
                Some(_) => {}
                None => {
                    base.extra_keywords.insert(key, value);
                }
            }
        }

        Ok(base)
    }

    fn collect_object_schema_view(
        &mut self,
        schema: &Schema,
        context: &str,
    ) -> Result<ObjectSchemaView> {
        let mut view = ObjectSchemaView::default();
        self.collect_object_schema_view_into(schema, context, &mut view)?;
        Ok(view)
    }

    fn collect_object_schema_view_into(
        &mut self,
        schema: &Schema,
        context: &str,
        view: &mut ObjectSchemaView,
    ) -> Result<()> {
        self.validate_schema_keywords(schema, context)?;

        if let Some(reference) = &schema.reference {
            if self
                .active_object_view_refs
                .iter()
                .any(|item| item == reference)
            {
                self.handle_unhandled(
                    context,
                    DiagnosticKind::AllOfRecursiveCycle {
                        reference: reference.clone(),
                    },
                )?;
                return Ok(());
            }

            self.active_object_view_refs.push(reference.clone());
            let resolved = self.resolve_schema_reference(reference)?;
            self.collect_object_schema_view_into(&resolved, reference, view)?;
            self.active_object_view_refs.pop();
        }

        if let Some(members) = &schema.all_of {
            for member in members {
                self.collect_object_schema_view_into(member, context, view)?;
            }
        }

        merge_non_codegen_optional_field(&mut view.title, schema.title.clone());

        if let Some(required) = &schema.required {
            view.required.extend(required.iter().cloned());
        }

        if let Some(properties) = &schema.properties {
            for (field_name, property_schema_or_bool) in properties {
                // Skip boolean schemas — they have no fields to contribute.
                let Some(property_schema) = property_schema_or_bool.as_schema() else {
                    continue;
                };
                if let Some(existing) = view.properties.shift_remove(field_name) {
                    view.properties.insert(
                        field_name.clone(),
                        self.merge_schemas(existing, property_schema.clone(), context)?,
                    );
                } else {
                    view.properties
                        .insert(field_name.clone(), property_schema.clone());
                }
            }
        }

        Ok(())
    }

    fn import_const_type(
        &mut self,
        schema: &Schema,
        const_value: &Value,
        context: &InlineModelContext,
    ) -> Result<ImportedType> {
        if let Some(schema_type) = schema.primary_schema_type() {
            let imported = match schema_type {
                "string" => {
                    if schema.format.as_deref() == Some("binary") {
                        ImportedType::plain(TypeRef::primitive("binary"))
                    } else {
                        ImportedType::plain(TypeRef::primitive("string"))
                    }
                }
                "integer" => ImportedType::plain(TypeRef::primitive("integer")),
                "number" => ImportedType::plain(TypeRef::primitive("number")),
                "boolean" => ImportedType::plain(TypeRef::primitive("boolean")),
                "null" => ImportedType {
                    type_ref: Some(TypeRef::primitive("any")),
                    nullable: true,
                },
                "array" => {
                    match schema.items.as_ref() {
                        Some(item_schema) => {
                            let imported = self.import_schema_type(item_schema, context)?;
                            ImportedType::plain(TypeRef::array(
                                imported
                                    .type_ref
                                    .unwrap_or_else(|| TypeRef::primitive("any")),
                            ))
                        }
                        // JSON Schema: array without `items` means array of any.
                        None => ImportedType::plain(TypeRef::array(TypeRef::primitive("any"))),
                    }
                }
                "object" => self.import_object_type(schema, context, None)?,
                other => {
                    self.handle_unhandled(
                        &context.describe(),
                        DiagnosticKind::UnsupportedSchemaType {
                            schema_type: other.to_owned(),
                        },
                    )?;
                    ImportedType::plain(TypeRef::primitive("any"))
                }
            };
            return Ok(imported);
        }

        let imported = match const_value {
            Value::String(_) => ImportedType::plain(TypeRef::primitive("string")),
            Value::Bool(_) => ImportedType::plain(TypeRef::primitive("boolean")),
            Value::Number(number) => {
                if number.is_i64() || number.is_u64() {
                    ImportedType::plain(TypeRef::primitive("integer"))
                } else {
                    ImportedType::plain(TypeRef::primitive("number"))
                }
            }
            Value::Null => ImportedType {
                type_ref: Some(TypeRef::primitive("any")),
                nullable: true,
            },
            Value::Array(_) => {
                if let Some(items) = &schema.items {
                    let imported = self.import_schema_type(items, context)?;
                    ImportedType::plain(TypeRef::array(
                        imported
                            .type_ref
                            .unwrap_or_else(|| TypeRef::primitive("any")),
                    ))
                } else {
                    ImportedType::plain(TypeRef::array(TypeRef::primitive("any")))
                }
            }
            Value::Object(_) => self.import_object_type(schema, context, None)?,
        };

        Ok(imported)
    }

    fn import_any_of(
        &mut self,
        schemas: &[Schema],
        context: &InlineModelContext,
    ) -> Result<ImportedType> {
        let mut variants = Vec::new();
        let mut nullable = false;

        for schema in schemas {
            if schema.is_exact_null_type() {
                nullable = true;
                continue;
            }

            let imported = self.import_schema_type(schema, context)?;
            if imported.nullable {
                nullable = true;
            }
            if let Some(type_ref) = imported.type_ref {
                variants.push(type_ref);
            }
        }

        variants = dedupe_variants(variants);
        let type_ref = match variants.len() {
            0 => Some(TypeRef::primitive("any")),
            1 => variants.into_iter().next(),
            _ => Some(TypeRef::Union { variants }),
        };

        Ok(ImportedType { type_ref, nullable })
    }

    fn import_object_type(
        &mut self,
        schema: &Schema,
        context: &InlineModelContext,
        local_reference: Option<&str>,
    ) -> Result<ImportedType> {
        if let Some(additional_properties) = &schema.additional_properties {
            match additional_properties {
                AdditionalProperties::Schema(additional_properties) => {
                    let imported = self.import_schema_type(additional_properties, context)?;
                    return Ok(ImportedType::plain(TypeRef::map(
                        imported
                            .type_ref
                            .unwrap_or_else(|| TypeRef::primitive("any")),
                    )));
                }
                AdditionalProperties::Bool(true) => {
                    return Ok(ImportedType::plain(TypeRef::map(TypeRef::primitive("any"))));
                }
                AdditionalProperties::Bool(false) => {}
            }
        }

        if schema.properties.is_some() {
            let model_name = if let Some(reference) = local_reference {
                self.local_ref_model_names
                    .get(reference)
                    .cloned()
                    .unwrap_or_else(|| {
                        let model_name = self.inline_model_name(schema, context);
                        self.local_ref_model_names
                            .insert(reference.to_owned(), model_name.clone());
                        model_name
                    })
            } else {
                self.inline_model_name(schema, context)
            };

            if self.models.contains_key(&model_name)
                || self.active_model_builds.contains(&model_name)
            {
                return Ok(ImportedType::plain(TypeRef::named(model_name)));
            }

            self.active_model_builds.insert(model_name.clone());
            if !self.models.contains_key(&model_name) {
                let pointer = context.synthetic_pointer(&model_name);
                let build_result = self.build_model_from_schema(&model_name, schema, &pointer);
                self.active_model_builds.remove(&model_name);
                let model = build_result?;
                self.generated_model_names.insert(model_name.clone());
                self.models.insert(model_name.clone(), model);
            } else {
                self.active_model_builds.remove(&model_name);
            }
            return Ok(ImportedType::plain(TypeRef::named(model_name)));
        }

        Ok(ImportedType::plain(TypeRef::primitive("object")))
    }

    fn inline_model_name(&mut self, schema: &Schema, context: &InlineModelContext) -> String {
        let base = schema.title.clone().unwrap_or_else(|| context.name_hint());
        let candidate = to_pascal_case(&base);
        if self.generated_model_names.insert(candidate.clone()) {
            return candidate;
        }

        let mut index = 2usize;
        loop {
            let candidate = format!("{candidate}{index}");
            if self.generated_model_names.insert(candidate.clone()) {
                return candidate;
            }
            index += 1;
        }
    }

    fn handle_unhandled(&mut self, context: &str, kind: DiagnosticKind) -> Result<()> {
        let diagnostic = self.make_diagnostic(context, kind);
        if self.options.ignore_unhandled {
            self.warnings.push(diagnostic);
            Ok(())
        } else {
            Err(anyhow::Error::new(diagnostic))
        }
    }

    /// Build an [`OpenApiDiagnostic`] from a context string (either a JSON
    /// pointer starting with `#/` or a human-readable label like
    /// `"parameter \`foo\`"`).
    fn make_diagnostic(&self, context: &str, kind: DiagnosticKind) -> OpenApiDiagnostic {
        if context.starts_with("#/") {
            let (preview, line) = self.source.pointer_info(context);
            OpenApiDiagnostic::from_pointer(kind, context, preview, line)
        } else {
            OpenApiDiagnostic::from_named_context(kind, context)
        }
    }

    /// Build a pointer diagnostic using the importer's source for preview
    /// rendering.
    fn make_pointer_diagnostic(&self, pointer: &str, kind: DiagnosticKind) -> OpenApiDiagnostic {
        let (preview, line) = self.source.pointer_info(pointer);
        OpenApiDiagnostic::from_pointer(kind, pointer, preview, line)
    }
}

#[derive(Debug)]
struct LoadedOpenApiDocument {
    document: OpenApiDocument,
    source: OpenApiSource,
}

#[derive(Debug)]
struct OpenApiSource {
    format: SourceFormat,
    raw: String,
    value: OnceLock<Option<Value>>,
    /// Exact pointer → 1-based line map, built lazily from the YAML event stream.
    /// `None` means the crate is JSON (uses heuristic instead) or YAML parsing failed.
    line_map: OnceLock<Option<HashMap<String, usize>>>,
}

#[derive(Debug, Clone, Copy)]
enum SourceFormat {
    Json,
    Yaml,
}

impl OpenApiSource {
    fn new(format: SourceFormat, raw: String) -> Self {
        Self {
            format,
            raw,
            value: OnceLock::new(),
            line_map: OnceLock::new(),
        }
    }

    fn render_pointer_preview(&self, pointer: &str) -> Option<String> {
        let node = self
            .value
            .get_or_init(|| self.parse_value())
            .as_ref()?
            .pointer(pointer.strip_prefix('#').unwrap_or(pointer))?;
        let rendered = match self.format {
            SourceFormat::Json => serde_json::to_string_pretty(node).ok()?,
            SourceFormat::Yaml => serde_yaml::to_string(node).ok()?,
        };
        Some(truncate_preview(&rendered, 10))
    }

    /// Return `(preview_string, 1_based_line)` for the node at `pointer`.
    ///
    /// For YAML sources both values are derived together: we look up the exact
    /// key line from the event-stream map and then slice the raw text from that
    /// line, so the stored line and the start of the preview are always the
    /// same point in the file.  For JSON sources we fall back to the
    /// serde-rendered preview and a text-search heuristic line number.
    fn pointer_info(&self, pointer: &str) -> (Option<String>, Option<usize>) {
        match self.format {
            SourceFormat::Yaml => {
                let key = pointer.strip_prefix('#').unwrap_or(pointer);
                let line = self
                    .line_map
                    .get_or_init(|| Some(build_yaml_line_map(&self.raw)))
                    .as_ref()
                    .and_then(|m| m.get(key).copied());
                let preview = line.map(|l| self.raw_preview_from_line(l));
                (preview, line)
            }
            SourceFormat::Json => {
                let preview = self.render_pointer_preview(pointer);
                let line = self.resolve_pointer_line_heuristic(pointer);
                (preview, line)
            }
        }
    }

    /// Slice `max_lines` raw source lines starting at 1-based `start_line`,
    /// dedented to remove the leading whitespace shared by all lines.
    fn raw_preview_from_line(&self, start_line: usize) -> String {
        const MAX_LINES: usize = 10;
        let lines: Vec<&str> = self
            .raw
            .lines()
            .skip(start_line.saturating_sub(1))
            .take(MAX_LINES)
            .collect();
        // Compute common leading-whitespace indent so the preview isn't
        // rendered with the full nesting depth.
        let indent = lines
            .iter()
            .filter(|l| !l.trim().is_empty())
            .map(|l| l.len() - l.trim_start().len())
            .min()
            .unwrap_or(0);
        let dedented: Vec<&str> = lines.iter().map(|l| &l[indent.min(l.len())..]).collect();
        dedented.join("\n")
    }
    /// Return the exact 1-based line number for the node identified by `pointer`.
    ///
    /// For YAML sources this is resolved via an exact pointer→line map produced
    /// by walking the YAML event stream (see [`build_yaml_line_map`]).  For JSON
    /// sources a best-effort text-search heuristic is used as a fallback.
    fn resolve_pointer_line(&self, pointer: &str) -> Option<usize> {
        // For YAML, pointer_info() is the unified entry point; this helper is
        // retained for callers that only need the line (e.g. make_diagnostic
        // routes through pointer_info directly).
        let key = pointer.strip_prefix('#').unwrap_or(pointer);
        match self.format {
            SourceFormat::Yaml => self
                .line_map
                .get_or_init(|| Some(build_yaml_line_map(&self.raw)))
                .as_ref()
                .and_then(|m| m.get(key).copied()),
            SourceFormat::Json => self.resolve_pointer_line_heuristic(pointer),
        }
    }

    /// Text-search heuristic used for JSON sources (or as a last-resort fallback).
    fn resolve_pointer_line_heuristic(&self, pointer: &str) -> Option<usize> {
        let inner = pointer.strip_prefix('#').unwrap_or(pointer);
        let segments: Vec<String> = inner
            .split('/')
            .filter(|s| !s.is_empty())
            .map(|s| s.replace("~1", "/").replace("~0", "~"))
            .collect();

        let lines: Vec<&str> = self.raw.lines().collect();
        let mut search_from = 0usize;
        let mut last_found: Option<usize> = None;

        for segment in &segments {
            let yaml_pat = format!("{}:", segment);
            let json_pat = format!("\"{}\":", segment);
            for (idx, line) in lines.iter().enumerate().skip(search_from) {
                let trimmed = line.trim();
                if trimmed.starts_with(&yaml_pat) || trimmed.starts_with(&json_pat) {
                    last_found = Some(idx + 1);
                    search_from = idx + 1;
                    break;
                }
            }
        }
        last_found
    }

    fn parse_value(&self) -> Option<Value> {
        match self.format {
            SourceFormat::Json => serde_json::from_str(&self.raw).ok(),
            SourceFormat::Yaml => {
                let yaml_value: serde_yaml::Value = serde_yaml::from_str(&self.raw).ok()?;
                serde_json::to_value(yaml_value).ok()
            }
        }
    }
}

fn truncate_preview(rendered: &str, max_lines: usize) -> String {
    let lines = rendered.lines().collect::<Vec<_>>();
    if lines.len() <= max_lines {
        return rendered.to_owned();
    }

    let mut output = lines
        .into_iter()
        .take(max_lines)
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    output.push("...".into());
    output.join("\n")
}

/// Build an exact JSON-pointer → 1-based-line map by walking the YAML event
/// stream.  Every key scalar's line is recorded under the pointer formed by
/// appending that key (RFC 6901-encoded) to the parent pointer.  This gives
/// precise, heuristic-free location data for any depth, including empty-string
/// keys and numeric array indices.
fn build_yaml_line_map(raw: &str) -> HashMap<String, usize> {
    use yaml_rust2::parser::{Event, MarkedEventReceiver, Parser};
    use yaml_rust2::scanner::Marker;

    enum Frame {
        Mapping {
            ptr: String,
            /// `true`  = next Scalar event is a key
            /// `false` = next event is the value for `pending_key`
            expecting_key: bool,
            pending_key: String,
            /// 1-based line of `pending_key`; used as the stored line for
            /// scalar values (key and value are on the same line).  Complex
            /// values (MappingStart/SequenceStart) use the event's own mark
            /// instead, which points at the first line of rendered content.
            pending_line: usize,
        },
        Sequence {
            ptr: String,
            index: usize,
        },
    }

    struct Collector {
        stack: Vec<Frame>,
        map: HashMap<String, usize>,
    }

    /// RFC 6901 segment encoding (~ → ~0, / → ~1).
    fn enc(key: &str) -> String {
        key.replace('~', "~0").replace('/', "~1")
    }

    impl MarkedEventReceiver for Collector {
        fn on_event(&mut self, ev: Event, mark: Marker) {
            // yaml-rust2 Marker::line() is already 1-based.
            let line = mark.line();

            match ev {
                // ── Mapping / Sequence start ───────────────────────────────
                Event::MappingStart(..) | Event::SequenceStart(..) => {
                    let is_mapping = matches!(ev, Event::MappingStart(..));

                    // Phase 1: derive the child pointer from the parent frame.
                    // We compute owned Strings so the borrow on self.stack ends
                    // before we mutate self.map / self.stack below.
                    let (child_ptr, record_line) = match self.stack.last() {
                        None => (String::new(), None), // root node
                        Some(Frame::Mapping {
                            ptr,
                            expecting_key: false,
                            pending_key,
                            pending_line,
                        }) => {
                            // Record the KEY's line (`pending_line`) so that
                            // `raw_preview_from_line` starts exactly at the key
                            // (e.g. `"":`), which is what users and the frontend
                            // expect to see highlighted.
                            (format!("{}/{}", ptr, enc(pending_key)), Some(*pending_line))
                        }
                        Some(Frame::Sequence { ptr, index }) => {
                            (format!("{}/{}", ptr, index), Some(line))
                        }
                        // expecting_key == true here would mean a nested
                        // structure used as a mapping key — invalid YAML.
                        _ => return,
                    };

                    // Phase 2: record, update parent, push child (no active
                    // borrow on self.stack from this point).
                    if let Some(l) = record_line {
                        self.map.insert(child_ptr.clone(), l);
                    }
                    match self.stack.last_mut() {
                        Some(Frame::Mapping { expecting_key, .. }) => *expecting_key = true,
                        Some(Frame::Sequence { index, .. }) => *index += 1,
                        None => {}
                    }
                    if is_mapping {
                        self.stack.push(Frame::Mapping {
                            ptr: child_ptr,
                            expecting_key: true,
                            pending_key: String::new(),
                            pending_line: 0,
                        });
                    } else {
                        self.stack.push(Frame::Sequence {
                            ptr: child_ptr,
                            index: 0,
                        });
                    }
                }

                // ── Mapping / Sequence end ────────────────────────────────
                Event::MappingEnd | Event::SequenceEnd => {
                    self.stack.pop();
                }

                // ── Scalar ────────────────────────────────────────────────
                Event::Scalar(value, ..) => {
                    // Phase 1: figure out what the scalar represents.
                    let is_key = matches!(
                        self.stack.last(),
                        Some(Frame::Mapping { expecting_key: true, .. })
                    );
                    let value_info: Option<(String, usize)> = if !is_key {
                        match self.stack.last() {
                            Some(Frame::Mapping {
                                ptr,
                                expecting_key: false,
                                pending_key,
                                pending_line,
                            }) => Some((format!("{}/{}", ptr, enc(pending_key)), *pending_line)),
                            Some(Frame::Sequence { ptr, index }) => {
                                Some((format!("{}/{}", ptr, index), line))
                            }
                            _ => None,
                        }
                    } else {
                        None
                    };

                    // Phase 2: apply (borrows above have been released).
                    if is_key {
                        if let Some(Frame::Mapping {
                            expecting_key,
                            pending_key,
                            pending_line,
                            ..
                        }) = self.stack.last_mut()
                        {
                            *pending_key = value;
                            *pending_line = line;
                            *expecting_key = false;
                        }
                    } else if let Some((child_ptr, record_line)) = value_info {
                        self.map.insert(child_ptr, record_line);
                        match self.stack.last_mut() {
                            Some(Frame::Mapping { expecting_key, .. }) => *expecting_key = true,
                            Some(Frame::Sequence { index, .. }) => *index += 1,
                            _ => {}
                        }
                    }
                }

                _ => {}
            }
        }
    }

    let mut collector = Collector {
        stack: Vec::new(),
        map: HashMap::new(),
    };
    let mut parser = Parser::new(raw.chars());
    if parser.load(&mut collector, false).is_err() {
        return HashMap::new();
    }
    collector.map
}

#[derive(Debug, Clone)]
struct ImportedType {
    type_ref: Option<TypeRef>,
    nullable: bool,
}

impl ImportedType {
    fn plain(type_ref: TypeRef) -> Self {
        Self {
            type_ref: Some(type_ref),
            nullable: false,
        }
    }
}

#[derive(Default)]
struct ObjectSchemaView {
    title: Option<String>,
    properties: IndexMap<String, Schema>,
    required: BTreeSet<String>,
}

#[derive(Debug)]
enum InlineModelContext {
    NamedSchema {
        name: String,
        pointer: String,
    },
    Field {
        model_name: String,
        field_name: String,
        pointer: String,
    },
    RequestBody {
        operation_name: String,
        pointer: String,
    },
    Response {
        operation_name: String,
        status: String,
        pointer: String,
    },
    Parameter {
        name: String,
    },
}

impl InlineModelContext {
    fn name_hint(&self) -> String {
        match self {
            Self::NamedSchema { name, .. } => name.clone(),
            Self::Field {
                model_name,
                field_name,
                ..
            } => format!("{model_name} {field_name}"),
            Self::RequestBody { operation_name, .. } => format!("{operation_name} request"),
            Self::Response {
                operation_name,
                status,
                ..
            } => format!("{operation_name} {status} response"),
            Self::Parameter { name } => format!("{name} param"),
        }
    }

    fn describe(&self) -> String {
        match self {
            InlineModelContext::NamedSchema { pointer, .. } => pointer.clone(),
            InlineModelContext::Field { pointer, .. } => pointer.clone(),
            InlineModelContext::RequestBody { pointer, .. } => pointer.clone(),
            InlineModelContext::Response { pointer, .. } => pointer.clone(),
            InlineModelContext::Parameter { name } => format!("parameter `{name}`"),
        }
    }

    fn synthetic_pointer(&self, model_name: &str) -> String {
        match self {
            Self::NamedSchema { pointer, .. } => pointer.clone(),
            Self::Field { pointer, .. } => pointer.clone(),
            Self::RequestBody { pointer, .. } => pointer.clone(),
            Self::Response { pointer, .. } => pointer.clone(),
            Self::Parameter { name } => format!("#/synthetic/parameters/{name}/{model_name}"),
        }
    }
}

/// Version-specific input struct for Swagger 2.0 documents.
/// Top-level `definitions`, `parameters`, `responses`, and `consumes` live here;
/// OpenAPI 3's `components` is absent.
#[derive(Debug, Deserialize, Clone)]
struct Swagger2Document {
    #[serde(default)]
    #[serde(deserialize_with = "deserialize_paths_map")]
    paths: BTreeMap<String, PathItem>,
    #[serde(default)]
    consumes: Vec<String>,
    #[serde(default)]
    parameters: BTreeMap<String, ParameterSpec>,
    #[serde(rename = "definitions")]
    #[serde(default)]
    definitions: BTreeMap<String, Schema>,
    #[serde(default)]
    responses: BTreeMap<String, ResponseSpec>,
}

/// Version-specific input struct for OpenAPI 3.x documents.
/// Top-level `definitions`, `consumes`, etc. are absent; everything lives under `components`.
#[derive(Debug, Deserialize, Clone)]
struct OpenApi3Document {
    #[serde(default)]
    #[serde(deserialize_with = "deserialize_paths_map")]
    paths: BTreeMap<String, PathItem>,
    #[serde(default)]
    components: Components,
}

/// Normalised internal document form fed to `OpenApiImporter`.
/// Retains `Deserialize` so unit-test helpers can construct it directly from inline JSON fixtures.
#[derive(Debug, Deserialize, Clone)]
struct OpenApiDocument {
    #[serde(default)]
    #[serde(deserialize_with = "deserialize_paths_map")]
    paths: BTreeMap<String, PathItem>,
    #[serde(default)]
    consumes: Vec<String>,
    #[serde(default)]
    parameters: BTreeMap<String, ParameterSpec>,
    #[serde(rename = "definitions")]
    #[serde(default)]
    definitions: BTreeMap<String, Schema>,
    #[serde(default)]
    responses: BTreeMap<String, ResponseSpec>,
    #[serde(default)]
    components: Components,
}

impl From<Swagger2Document> for OpenApiDocument {
    fn from(doc: Swagger2Document) -> Self {
        Self {
            paths: doc.paths,
            consumes: doc.consumes,
            parameters: doc.parameters,
            definitions: doc.definitions,
            responses: doc.responses,
            components: Components::default(),
        }
    }
}

impl From<OpenApi3Document> for OpenApiDocument {
    fn from(doc: OpenApi3Document) -> Self {
        Self {
            paths: doc.paths,
            consumes: Vec::new(),
            parameters: BTreeMap::new(),
            definitions: BTreeMap::new(),
            responses: BTreeMap::new(),
            components: doc.components,
        }
    }
}

#[derive(Debug, Deserialize, Default, Clone)]
struct Components {
    #[serde(default)]
    schemas: BTreeMap<String, Schema>,
    #[serde(default)]
    parameters: BTreeMap<String, ParameterSpec>,
    #[serde(rename = "requestBodies")]
    #[serde(default)]
    request_bodies: BTreeMap<String, RequestBodyOrRef>,
    #[serde(default)]
    responses: BTreeMap<String, ResponseSpec>,
}

#[derive(Debug, Deserialize, Default, Clone)]
struct PathItem {
    #[serde(default)]
    parameters: Option<Vec<ParameterOrRef>>,
    #[serde(default)]
    get: Option<OperationSpec>,
    #[serde(default)]
    post: Option<OperationSpec>,
    #[serde(default)]
    put: Option<OperationSpec>,
    #[serde(default)]
    patch: Option<OperationSpec>,
    #[serde(default)]
    delete: Option<OperationSpec>,
}

#[derive(Debug, Deserialize, Default, Clone)]
struct OperationSpec {
    #[serde(rename = "operationId")]
    #[serde(default)]
    operation_id: Option<String>,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    parameters: Vec<ParameterOrRef>,
    #[serde(default)]
    consumes: Vec<String>,
    #[serde(rename = "requestBody")]
    #[serde(default)]
    request_body: Option<RequestBodyOrRef>,
    #[serde(default)]
    responses: BTreeMap<String, ResponseSpecOrRef>,
}

#[derive(Debug, Deserialize, Clone)]
struct ParameterSpec {
    name: String,
    #[serde(default)]
    description: String,
    #[serde(rename = "in")]
    location: RawParameterLocation,
    #[serde(default)]
    required: bool,
    #[serde(default)]
    schema: Option<Schema>,
    #[serde(rename = "type")]
    #[serde(default)]
    parameter_type: Option<SchemaTypeDecl>,
    #[serde(default)]
    format: Option<String>,
    #[serde(default)]
    items: Option<Box<Schema>>,
    #[serde(rename = "collectionFormat")]
    #[serde(default)]
    collection_format: Option<String>,
    /// OpenAPI 3 alternative to `schema`: a single-entry media-type map.
    #[serde(default)]
    content: BTreeMap<String, MediaTypeSpec>,
}

impl ParameterSpec {
    fn effective_schema(&self) -> Option<Schema> {
        self.schema
            .clone()
            .or_else(|| {
                self.parameter_type.clone().map(|schema_type| Schema {
                    schema_type: Some(schema_type),
                    format: self.format.clone(),
                    items: self.items.clone(),
                    ..Schema::default()
                })
            })
            .or_else(|| {
                // OpenAPI 3 allows `content` instead of `schema` on parameters.
                // Use the schema from the first (and per-spec, only) entry.
                self.content
                    .values()
                    .next()
                    .and_then(|media| media.schema.clone())
            })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RawParameterLocation {
    Path,
    Query,
    Header,
    Cookie,
    Body,
    FormData,
}

impl RawParameterLocation {
    fn as_ir_location(self) -> Option<ParameterLocation> {
        match self {
            Self::Path => Some(ParameterLocation::Path),
            Self::Query => Some(ParameterLocation::Query),
            Self::Header => Some(ParameterLocation::Header),
            Self::Cookie => Some(ParameterLocation::Cookie),
            Self::Body | Self::FormData => None,
        }
    }
}

impl<'de> Deserialize<'de> for RawParameterLocation {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        match value.as_str() {
            "path" => Ok(Self::Path),
            "query" => Ok(Self::Query),
            "header" => Ok(Self::Header),
            "cookie" => Ok(Self::Cookie),
            "body" => Ok(Self::Body),
            "formData" | "formdata" => Ok(Self::FormData),
            _ => Err(serde::de::Error::unknown_variant(
                &value,
                &["path", "query", "header", "cookie", "body", "formData"],
            )),
        }
    }
}

fn raw_parameter_location_label(location: RawParameterLocation) -> &'static str {
    match location {
        RawParameterLocation::Path => "path",
        RawParameterLocation::Query => "query",
        RawParameterLocation::Header => "header",
        RawParameterLocation::Cookie => "cookie",
        RawParameterLocation::Body => "body",
        RawParameterLocation::FormData => "form_data",
    }
}

#[derive(Debug, Deserialize, Clone)]
#[serde(untagged)]
enum ParameterOrRef {
    Ref {
        #[serde(rename = "$ref")]
        reference: String,
    },
    Inline(ParameterSpec),
}

#[derive(Debug, Deserialize, Default, Clone)]
struct RequestBodySpec {
    #[serde(default)]
    required: bool,
    #[serde(default)]
    content: BTreeMap<String, MediaTypeSpec>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(untagged)]
enum RequestBodyOrRef {
    Ref {
        #[serde(rename = "$ref")]
        reference: String,
    },
    Inline(RequestBodySpec),
}

/// A response entry that may be either an inline spec or a `$ref` pointer.
/// Using a flat struct (rather than an untagged enum) ensures that
/// `serde_path_to_error` can track the full JSON/YAML path through the
/// struct's fields, giving accurate error locations on parse failure.
#[derive(Debug, Deserialize, Default, Clone)]
struct ResponseSpecOrRef {
    #[serde(rename = "$ref")]
    #[serde(default)]
    reference: Option<String>,
    #[serde(default)]
    description: String,
    #[serde(default)]
    content: BTreeMap<String, MediaTypeSpec>,
}

#[derive(Debug, Deserialize, Default, Clone)]
struct ResponseSpec {
    #[serde(default)]
    description: String,
    #[serde(default)]
    content: BTreeMap<String, MediaTypeSpec>,
}

#[derive(Debug, Deserialize, Default, Clone)]
struct MediaTypeSpec {
    #[serde(default)]
    schema: Option<Schema>,
}

#[derive(Debug, Deserialize, Default, Clone, PartialEq)]
struct Schema {
    #[serde(rename = "$ref")]
    #[serde(default)]
    reference: Option<String>,
    #[serde(default)]
    definitions: Option<BTreeMap<String, Schema>>,
    #[serde(rename = "type")]
    #[serde(default)]
    schema_type: Option<SchemaTypeDecl>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    format: Option<String>,
    #[serde(rename = "const")]
    #[serde(default)]
    const_value: Option<Value>,
    #[serde(rename = "discriminator")]
    #[serde(default)]
    _discriminator: Option<Value>,
    #[serde(rename = "allOf")]
    #[serde(default)]
    all_of: Option<Vec<Schema>>,
    #[serde(rename = "enum")]
    #[serde(default)]
    enum_values: Option<Vec<Value>>,
    #[serde(default)]
    properties: Option<IndexMap<String, SchemaOrBool>>,
    #[serde(default)]
    required: Option<Vec<String>>,
    #[serde(default)]
    items: Option<Box<Schema>>,
    #[serde(rename = "additionalProperties")]
    #[serde(default)]
    additional_properties: Option<AdditionalProperties>,
    #[serde(rename = "anyOf")]
    #[serde(default)]
    any_of: Option<Vec<Schema>>,
    #[serde(rename = "oneOf")]
    #[serde(default)]
    one_of: Option<Vec<Schema>>,
    // Capture numeric constraint keywords explicitly to avoid serde_yaml integer
    // coercion failures that occur when these pass through the flattened map.
    #[serde(default)]
    minimum: Option<Value>,
    #[serde(default)]
    maximum: Option<Value>,
    #[serde(rename = "exclusiveMinimum")]
    #[serde(default)]
    exclusive_minimum: Option<Value>,
    #[serde(rename = "exclusiveMaximum")]
    #[serde(default)]
    exclusive_maximum: Option<Value>,
    #[serde(default)]
    #[serde(rename = "multipleOf")]
    multiple_of: Option<Value>,
    #[serde(default)]
    #[serde(rename = "minLength")]
    min_length: Option<Value>,
    #[serde(default)]
    #[serde(rename = "maxLength")]
    max_length: Option<Value>,
    #[serde(default)]
    #[serde(rename = "minItems")]
    min_items: Option<Value>,
    #[serde(default)]
    #[serde(rename = "maxItems")]
    max_items: Option<Value>,
    #[serde(default)]
    #[serde(rename = "minProperties")]
    min_properties: Option<Value>,
    #[serde(default)]
    #[serde(rename = "maxProperties")]
    max_properties: Option<Value>,
    #[serde(flatten)]
    #[serde(default)]
    extra_keywords: BTreeMap<String, Value>,
}

#[derive(Debug, Deserialize, Clone, PartialEq)]
#[serde(untagged)]
enum AdditionalProperties {
    Bool(bool),
    Schema(Box<Schema>),
}

/// A property schema that may be a full schema object or a boolean schema
/// (valid in OpenAPI 3.1 / JSON Schema: `false` = never valid, `true` = always valid).
/// Boolean schemas are treated as absent properties for code-generation purposes.
#[derive(Debug, Clone, PartialEq)]
enum SchemaOrBool {
    Schema(Schema),
    Bool(bool),
}

impl Default for SchemaOrBool {
    fn default() -> Self {
        SchemaOrBool::Schema(Schema::default())
    }
}

impl SchemaOrBool {
    /// Returns the inner schema, or `None` for boolean schemas.
    fn as_schema(&self) -> Option<&Schema> {
        match self {
            SchemaOrBool::Schema(s) => Some(s),
            SchemaOrBool::Bool(_) => None,
        }
    }
    fn into_schema(self) -> Option<Schema> {
        match self {
            SchemaOrBool::Schema(s) => Some(s),
            SchemaOrBool::Bool(_) => None,
        }
    }
}

impl<'de> serde::Deserialize<'de> for SchemaOrBool {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct SchemaOrBoolVisitor;
        impl<'de> serde::de::Visitor<'de> for SchemaOrBoolVisitor {
            type Value = SchemaOrBool;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "a JSON Schema object or boolean")
            }
            // Boolean schemas: `false` = never valid, `true` = always valid.
            fn visit_bool<E: serde::de::Error>(self, v: bool) -> std::result::Result<SchemaOrBool, E> {
                Ok(SchemaOrBool::Bool(v))
            }
            // Map: deserialize as a full Schema.  Using MapAccessDeserializer keeps
            // the serde_path_to_error-wrapped MapAccess in play so field-level
            // errors within the schema are tracked correctly.
            fn visit_map<A: serde::de::MapAccess<'de>>(self, map: A) -> std::result::Result<SchemaOrBool, A::Error> {
                let schema = Schema::deserialize(serde::de::value::MapAccessDeserializer::new(map))?;
                Ok(SchemaOrBool::Schema(schema))
            }
        }
        deserializer.deserialize_any(SchemaOrBoolVisitor)
    }
}

#[derive(Debug, Deserialize, Clone, PartialEq)]
#[serde(untagged)]
enum SchemaTypeDecl {
    Single(String),
    Multiple(Vec<String>),
    Embedded(Box<Schema>),
}

impl SchemaTypeDecl {
    fn as_slice(&self) -> &[String] {
        match self {
            Self::Single(value) => std::slice::from_ref(value),
            Self::Multiple(values) => values.as_slice(),
            Self::Embedded(_) => &[],
        }
    }

    fn embedded_schema(&self) -> Option<&Schema> {
        match self {
            Self::Embedded(schema) => Some(schema.as_ref()),
            _ => None,
        }
    }
}

impl Schema {
    fn schema_type_variants(&self) -> Option<&[String]> {
        self.schema_type.as_ref().map(SchemaTypeDecl::as_slice)
    }

    fn primary_schema_type(&self) -> Option<&str> {
        self.schema_type_variants()?
            .iter()
            .find(|value| value.as_str() != "null")
            .map(String::as_str)
    }

    fn is_exact_null_type(&self) -> bool {
        matches!(self.schema_type_variants(), Some([value]) if value == "null")
    }
}

fn ref_name(reference: &str) -> Result<String> {
    reference
        .rsplit('/')
        .next()
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| anyhow!("unsupported reference `{reference}`"))
}

fn is_named_schema_reference(reference: &str) -> bool {
    let Some(pointer) = reference.strip_prefix("#/") else {
        return false;
    };
    let segments = pointer.split('/').collect::<Vec<_>>();
    matches!(
        segments.as_slice(),
        ["components", "schemas", _] | ["definitions", _]
    )
}

fn is_inline_local_schema_reference(reference: &str) -> bool {
    reference.starts_with("#/") && !is_named_schema_reference(reference)
}

fn decode_json_pointer_segment(segment: &str) -> Result<String> {
    let unescaped = segment.replace("~1", "/").replace("~0", "~");
    percent_decode(&unescaped)
}

fn percent_decode(value: &str) -> Result<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() {
                bail!("unsupported reference segment `{value}`");
            }
            let high = (bytes[index + 1] as char)
                .to_digit(16)
                .ok_or_else(|| anyhow!("unsupported reference segment `{value}`"))?;
            let low = (bytes[index + 2] as char)
                .to_digit(16)
                .ok_or_else(|| anyhow!("unsupported reference segment `{value}`"))?;
            decoded.push(((high << 4) | low) as u8);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }

    String::from_utf8(decoded).map_err(|_| anyhow!("unsupported reference segment `{value}`"))
}

/// Resolve a `$ref` that points into a `ResponseSpec`, optionally continuing
/// into `content/{media_type}/schema/...`.
fn resolve_response_schema_reference(
    response: &ResponseSpec,
    segments: &[String],
    reference: &str,
) -> Result<Schema> {
    match segments {
        // Referencing the response object itself — use its primary schema.
        // If the response has no content (e.g. a description-only response used
        // mistakenly as a schema $ref), return an empty schema so callers treat
        // this as `any` rather than failing.
        [] => {
            let schema = response
                .content
                .values()
                .find_map(|media| media.schema.as_ref())
                .cloned()
                .unwrap_or_default();
            Ok(schema)
        }
        // content/{media_type}/schema/...
        [content_key, media_type, schema_key, rest @ ..]
            if content_key == "content" && schema_key == "schema" =>
        {
            let schema = response
                .content
                .get(media_type)
                .and_then(|media| media.schema.as_ref())
                .ok_or_else(|| anyhow!("unsupported reference `{reference}`"))?;
            resolve_nested_schema_reference(schema, rest, reference)
        }
        _ => Err(anyhow!("unsupported reference `{reference}`")),
    }
}

fn resolve_nested_schema_reference(
    schema: &Schema,
    segments: &[String],
    reference: &str,
) -> Result<Schema> {
    if segments.is_empty() {
        return Ok(schema.clone());
    }

    match segments {
        [segment, name, remainder @ ..] if segment == "definitions" => {
            let nested = schema
                .definitions
                .as_ref()
                .and_then(|definitions| definitions.get(name))
                .ok_or_else(|| anyhow!("unsupported reference `{reference}`"))?;
            resolve_nested_schema_reference(nested, remainder, reference)
        }
        [segment, remainder @ ..] if segment == "allOf" => {
            let index = remainder
                .first()
                .and_then(|value| value.parse::<usize>().ok())
                .ok_or_else(|| anyhow!("unsupported reference `{reference}`"))?;
            let member = schema
                .all_of
                .as_ref()
                .and_then(|members| members.get(index))
                .ok_or_else(|| anyhow!("unsupported reference `{reference}`"))?;
            resolve_nested_schema_reference(member, &remainder[1..], reference)
        }
        [segment, remainder @ ..] if segment == "anyOf" => {
            let index = remainder
                .first()
                .and_then(|value| value.parse::<usize>().ok())
                .ok_or_else(|| anyhow!("unsupported reference `{reference}`"))?;
            let member = schema
                .any_of
                .as_ref()
                .and_then(|members| members.get(index))
                .ok_or_else(|| anyhow!("unsupported reference `{reference}`"))?;
            resolve_nested_schema_reference(member, &remainder[1..], reference)
        }
        [segment, remainder @ ..] if segment == "oneOf" => {
            let index = remainder
                .first()
                .and_then(|value| value.parse::<usize>().ok())
                .ok_or_else(|| anyhow!("unsupported reference `{reference}`"))?;
            let member = schema
                .one_of
                .as_ref()
                .and_then(|members| members.get(index))
                .ok_or_else(|| anyhow!("unsupported reference `{reference}`"))?;
            resolve_nested_schema_reference(member, &remainder[1..], reference)
        }
        [segment, name, remainder @ ..] if segment == "properties" => {
            // Try top-level properties first.
            if let Some(property) = schema
                .properties
                .as_ref()
                .and_then(|p| p.get(name))
                .and_then(SchemaOrBool::as_schema)
            {
                return resolve_nested_schema_reference(property, remainder, reference);
            }
            // If the schema uses allOf with no top-level properties (e.g. a schema
            // whose properties are spread across its allOf members), search members.
            if let Some(all_of) = &schema.all_of {
                for member in all_of {
                    if let Some(property) = member
                        .properties
                        .as_ref()
                        .and_then(|p| p.get(name))
                        .and_then(SchemaOrBool::as_schema)
                    {
                        return resolve_nested_schema_reference(property, remainder, reference);
                    }
                }
            }
            Err(anyhow!("unsupported reference `{reference}`"))
        }
        [segment, remainder @ ..] if segment == "items" => {
            let item = schema
                .items
                .as_deref()
                .ok_or_else(|| anyhow!("unsupported reference `{reference}`"))?;
            resolve_nested_schema_reference(item, remainder, reference)
        }
        [segment, remainder @ ..] if segment == "additionalProperties" => {
            let nested = match schema.additional_properties.as_ref() {
                Some(AdditionalProperties::Schema(schema)) => schema.as_ref(),
                _ => return Err(anyhow!("unsupported reference `{reference}`")),
            };
            resolve_nested_schema_reference(nested, remainder, reference)
        }
        _ => Err(anyhow!("unsupported reference `{reference}`")),
    }
}

fn schema_is_object_like(schema: &Schema) -> bool {
    schema
        .schema_type_variants()
        .is_some_and(|variants| variants.iter().any(|value| value == "object"))
        || schema.properties.is_some()
        || schema.additional_properties.is_some()
}

fn is_validation_only_schema_variant(schema: &Schema) -> bool {
    schema.reference.is_none()
        && schema.definitions.is_none()
        && schema
            .schema_type
            .as_ref()
            .is_none_or(|decl| matches!(decl.as_slice(), [value] if value == "object"))
        && schema.format.is_none()
        && schema.const_value.is_none()
        && schema._discriminator.is_none()
        && schema.all_of.is_none()
        && schema.enum_values.is_none()
        && schema.properties.is_none()
        && schema.items.is_none()
        && schema.additional_properties.is_none()
        && schema.any_of.is_none()
        && schema.one_of.is_none()
        && schema
            .extra_keywords
            .keys()
            .all(|keyword| is_known_ignored_schema_keyword(keyword) || keyword.starts_with("x-"))
}

fn is_generic_object_placeholder(schema: &Schema) -> bool {
    let has_object_type = schema
        .schema_type
        .as_ref()
        .is_some_and(|decl| matches!(decl.as_slice(), [value] if value == "object"));

    (has_object_type || schema.properties.is_some())
        && schema
            .properties
            .as_ref()
            .is_some_and(|properties| properties.is_empty())
        && schema.additional_properties.is_none()
        && schema.definitions.is_none()
        && schema.items.is_none()
        && schema.enum_values.is_none()
        && schema.const_value.is_none()
        && schema.any_of.is_none()
        && schema.one_of.is_none()
        && schema.all_of.is_none()
        && schema._discriminator.is_none()
}

fn schema_runtime_attributes(schema: &Schema) -> Attributes {
    let mut attributes = Attributes::default();
    if let Some(description) = schema
        .extra_keywords
        .get("description")
        .and_then(Value::as_str)
    {
        attributes.insert("description".into(), Value::String(description.to_owned()));
    }
    if let Some(content_encoding) = schema
        .extra_keywords
        .get("contentEncoding")
        .and_then(Value::as_str)
    {
        attributes.insert(
            "content_encoding".into(),
            Value::String(content_encoding.to_owned()),
        );
    }
    if let Some(content_media_type) = schema
        .extra_keywords
        .get("contentMediaType")
        .and_then(Value::as_str)
    {
        attributes.insert(
            "content_media_type".into(),
            Value::String(content_media_type.to_owned()),
        );
    }
    attributes
}

fn parameter_attributes(param: &ParameterSpec, schema: &Schema) -> Attributes {
    let mut attributes = schema_runtime_attributes(schema);
    if !param.description.trim().is_empty() {
        attributes.insert(
            "description".into(),
            Value::String(param.description.trim().to_owned()),
        );
    }
    if let Some(collection_format) = &param.collection_format {
        attributes.insert(
            "collection_format".into(),
            Value::String(collection_format.clone()),
        );
    }
    attributes
}

fn is_unconstrained_schema(schema: &Schema) -> bool {
    schema.reference.is_none()
        && schema.definitions.is_none()
        && schema.schema_type.is_none()
        && schema.format.is_none()
        && schema.const_value.is_none()
        && schema._discriminator.is_none()
        && schema.all_of.is_none()
        && schema.enum_values.is_none()
        && schema.properties.is_none()
        && schema.required.is_none()
        && schema.items.is_none()
        && schema.additional_properties.is_none()
        && schema.any_of.is_none()
        && schema.one_of.is_none()
        && schema
            .extra_keywords
            .keys()
            .all(|keyword| is_known_ignored_schema_keyword(keyword) || keyword.starts_with("x-"))
}

fn schema_has_non_all_of_shape(schema: &Schema) -> bool {
    schema.reference.is_some()
        || schema.definitions.is_some()
        || schema.schema_type.is_some()
        || schema.format.is_some()
        || schema.const_value.is_some()
        || schema.enum_values.is_some()
        || schema.properties.is_some()
        || schema.required.is_some()
        || schema.items.is_some()
        || schema.additional_properties.is_some()
        || schema.any_of.is_some()
        || schema.one_of.is_some()
        || schema._discriminator.is_some()
}

fn is_known_ignored_schema_keyword(keyword: &str) -> bool {
    matches!(
        keyword,
        "default"
            | "not"
            | "description"
            | "example"
            | "examples"
            | "collectionFormat"
            | "contentEncoding"
            | "contentMediaType"
            | "externalDocs"
            | "xml"
            | "deprecated"
            | "readOnly"
            | "writeOnly"
            | "minimum"
            | "maximum"
            | "exclusiveMinimum"
            | "exclusiveMaximum"
            | "multipleOf"
            | "minLength"
            | "maxLength"
            | "pattern"
            | "minItems"
            | "maxItems"
            | "uniqueItems"
            | "minProperties"
            | "maxProperties"
            | "nullable"
            | "$schema"
            | "$id"
            | "$comment"
    )
}

fn is_known_but_unimplemented_schema_keyword(keyword: &str) -> bool {
    matches!(
        keyword,
        "if" | "then"
            | "else"
            | "contains"
            | "prefixItems"
            | "patternProperties"
            | "propertyNames"
            | "dependentSchemas"
            | "unevaluatedProperties"
            | "unevaluatedItems"
            | "$defs"
    )
}

fn fallback_operation_name(method: HttpMethod, path: &str) -> String {
    to_snake_case(&format!("{} {}", method_key(method), path))
}

fn method_key(method: HttpMethod) -> &'static str {
    match method {
        HttpMethod::Get => "get",
        HttpMethod::Post => "post",
        HttpMethod::Put => "put",
        HttpMethod::Patch => "patch",
        HttpMethod::Delete => "delete",
    }
}

fn operation_attributes(spec: &OperationSpec) -> Attributes {
    let mut attributes = Attributes::default();
    if let Some(summary) = &spec.summary {
        attributes.insert("summary".into(), Value::String(summary.clone()));
    }
    if !spec.tags.is_empty() {
        attributes.insert("tags".into(), json!(spec.tags));
    }
    attributes
}

fn json_pointer_key(input: &str) -> String {
    input.replace('~', "~0").replace('/', "~1")
}

fn to_pascal_case(input: &str) -> String {
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

fn to_snake_case(input: &str) -> String {
    let parts = split_words(input);
    if parts.is_empty() {
        return "value".into();
    }
    parts.join("_").to_lowercase()
}

fn split_words(input: &str) -> Vec<String> {
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

fn dedupe_variants(variants: Vec<TypeRef>) -> Vec<TypeRef> {
    let mut seen = BTreeSet::new();
    let mut deduped = Vec::new();
    for variant in variants {
        let key = serde_json::to_string(&variant).expect("type refs should always serialize");
        if seen.insert(key) {
            deduped.push(variant);
        }
    }
    deduped
}

fn merge_required(mut left: Vec<String>, right: Vec<String>) -> Vec<String> {
    let mut seen = left.iter().cloned().collect::<BTreeSet<_>>();
    for value in right {
        if seen.insert(value.clone()) {
            left.push(value);
        }
    }
    left
}

fn merge_optional_field<T>(
    target: &mut Option<T>,
    incoming: Option<T>,
    field_name: &str,
    context: &str,
    importer: &mut OpenApiImporter,
) -> Result<()>
where
    T: PartialEq,
{
    match (target.as_ref(), incoming) {
        (_, None) => {}
        (None, Some(value)) => *target = Some(value),
        (Some(existing), Some(value)) if *existing == value => {}
        (Some(_), Some(_)) => {
            importer.handle_unhandled(
                context,
                DiagnosticKind::IncompatibleAllOfField {
                    field: field_name.to_owned(),
                },
            )?;
        }
    }
    Ok(())
}

fn merge_non_codegen_optional_field<T>(target: &mut Option<T>, incoming: Option<T>) {
    if target.is_none() {
        *target = incoming;
    }
}

fn merge_schema_types(
    inferred_left: Option<SchemaTypeDecl>,
    inferred_right: Option<SchemaTypeDecl>,
    left_is_generic_object_placeholder: bool,
    right_is_generic_object_placeholder: bool,
    left: Option<SchemaTypeDecl>,
    right: Option<SchemaTypeDecl>,
    _context: &str,
    _importer: &mut OpenApiImporter,
) -> Result<Option<SchemaTypeDecl>> {
    match (left, right) {
        (None, None) => Ok(inferred_left.or(inferred_right)),
        (Some(value), None) => Ok(Some(value)),
        (None, Some(value)) => Ok(Some(value)),
        (Some(left), Some(right)) if left == right => Ok(Some(left)),
        (Some(left), Some(right)) => {
            let left_inferred = inferred_left.unwrap_or(left.clone());
            let right_inferred = inferred_right.unwrap_or(right.clone());
            if left_is_generic_object_placeholder {
                return Ok(Some(right_inferred));
            }
            if right_is_generic_object_placeholder {
                return Ok(Some(left_inferred));
            }
            if let Some(merged) =
                merge_numeric_compatible_schema_types(&left_inferred, &right_inferred)
            {
                return Ok(Some(merged));
            }
            if let Some(merged) =
                merge_nullable_compatible_schema_types(&left_inferred, &right_inferred)
            {
                return Ok(Some(merged));
            }
            if left_inferred == right_inferred {
                Ok(Some(left_inferred))
            } else {
                // Incompatible types in allOf: keep the left (base) type and continue.
                Ok(Some(left_inferred))
            }
        }
    }
}

fn merge_numeric_compatible_schema_types(
    left: &SchemaTypeDecl,
    right: &SchemaTypeDecl,
) -> Option<SchemaTypeDecl> {
    let left_variants = left.as_slice();
    let right_variants = right.as_slice();
    let left_has_numeric = left_variants
        .iter()
        .any(|value| value == "integer" || value == "number");
    let right_has_numeric = right_variants
        .iter()
        .any(|value| value == "integer" || value == "number");
    if !left_has_numeric || !right_has_numeric {
        return None;
    }

    let left_other = left_variants
        .iter()
        .filter(|value| value.as_str() != "integer" && value.as_str() != "number")
        .collect::<BTreeSet<_>>();
    let right_other = right_variants
        .iter()
        .filter(|value| value.as_str() != "integer" && value.as_str() != "number")
        .collect::<BTreeSet<_>>();
    if left_other != right_other {
        return None;
    }

    let mut merged = left_other
        .into_iter()
        .map(|value| value.to_owned())
        .collect::<Vec<_>>();
    merged.push("number".into());

    Some(if merged.len() == 1 {
        SchemaTypeDecl::Single(merged.remove(0))
    } else {
        SchemaTypeDecl::Multiple(merged)
    })
}

fn merge_nullable_compatible_schema_types(
    left: &SchemaTypeDecl,
    right: &SchemaTypeDecl,
) -> Option<SchemaTypeDecl> {
    let left_variants = left.as_slice();
    let right_variants = right.as_slice();
    if left_variants.is_empty() || right_variants.is_empty() {
        return None;
    }

    let left_has_null = left_variants.iter().any(|value| value == "null");
    let right_has_null = right_variants.iter().any(|value| value == "null");
    if !left_has_null && !right_has_null {
        return None;
    }

    let left_without_null = left_variants
        .iter()
        .filter(|value| value.as_str() != "null")
        .cloned()
        .collect::<BTreeSet<_>>();
    let right_without_null = right_variants
        .iter()
        .filter(|value| value.as_str() != "null")
        .cloned()
        .collect::<BTreeSet<_>>();

    let merged_without_null = if left_without_null.is_empty() && !right_without_null.is_empty() {
        right_without_null
    } else if right_without_null.is_empty() && !left_without_null.is_empty() {
        left_without_null
    } else if left_without_null == right_without_null {
        left_without_null
    } else {
        return None;
    };

    let mut merged = merged_without_null.into_iter().collect::<Vec<_>>();
    merged.push("null".into());

    Some(if merged.len() == 1 {
        SchemaTypeDecl::Single(merged.remove(0))
    } else {
        SchemaTypeDecl::Multiple(merged)
    })
}

fn merge_enum_values(
    left: Option<Vec<Value>>,
    right: Option<Vec<Value>>,
    _context: &str,
    _importer: &mut OpenApiImporter,
) -> Result<Option<Vec<Value>>> {
    match (left, right) {
        (None, None) => Ok(None),
        (Some(values), None) | (None, Some(values)) => Ok(Some(values)),
        (Some(left_values), Some(right_values)) => {
            let right_keys = right_values
                .iter()
                .map(serde_json::to_string)
                .collect::<std::result::Result<BTreeSet<_>, _>>()
                .expect("enum values should always serialize");
            let merged = left_values
                .iter()
                .filter(|value| {
                    let key =
                        serde_json::to_string(value).expect("enum values should always serialize");
                    right_keys.contains(&key)
                })
                .cloned()
                .collect::<Vec<_>>();

            // If the intersection is empty the enum sets are disjoint.
            // Accept all values from the left side as a graceful fallback.
            let result = if merged.is_empty() {
                left_values
            } else {
                merged
            };

            Ok(Some(result))
        }
    }
}

fn infer_schema_type_for_merge(schema: &Schema) -> Option<SchemaTypeDecl> {
    schema.schema_type.clone().or_else(|| {
        if schema.properties.is_some() || schema.additional_properties.is_some() {
            Some(SchemaTypeDecl::Single("object".into()))
        } else if schema.items.is_some() {
            Some(SchemaTypeDecl::Single("array".into()))
        } else if let Some(enum_values) = &schema.enum_values {
            match infer_enum_type(enum_values, schema.format.as_deref()) {
                TypeRef::Primitive { name } => Some(SchemaTypeDecl::Single(name)),
                _ => None,
            }
        } else {
            infer_format_only_type(schema.format.as_deref()).and_then(|type_ref| match type_ref {
                TypeRef::Primitive { name } => Some(SchemaTypeDecl::Single(name)),
                _ => None,
            })
        }
    })
}

fn infer_enum_type(enum_values: &[Value], format: Option<&str>) -> TypeRef {
    let inferred_name = if enum_values.iter().all(Value::is_string) {
        if format == Some("binary") {
            "binary"
        } else {
            "string"
        }
    } else if enum_values.iter().all(|value| value.as_i64().is_some()) {
        "integer"
    } else if enum_values.iter().all(Value::is_number) {
        "number"
    } else if enum_values.iter().all(Value::is_boolean) {
        "boolean"
    } else {
        "any"
    };

    TypeRef::primitive(inferred_name)
}

fn infer_format_only_type(format: Option<&str>) -> Option<TypeRef> {
    let inferred = match format? {
        "binary" => "binary",
        // Allow the primitive type names themselves used as format values.
        "boolean" | "bool" => "boolean",
        "integer" | "int" | "int32" | "int64" => "integer",
        "number" | "float" | "double" | "decimal" => "number",
        // "string" (and related) as format → infer string type.
        "string" | "byte" | "date" | "date-time" | "duration" | "email" | "hostname"
        | "host-name" | "ipv4" | "ipv6" | "password" | "uri" | "uuid" => "string",
        _ => return None,
    };
    Some(TypeRef::primitive(inferred))
}

fn merge_properties(
    importer: &mut OpenApiImporter,
    mut left: IndexMap<String, SchemaOrBool>,
    right: IndexMap<String, SchemaOrBool>,
    context: &str,
) -> Result<IndexMap<String, SchemaOrBool>> {
    for (key, value) in right {
        if let Some(existing) = left.shift_remove(&key) {
            let merged = match (existing, value) {
                (SchemaOrBool::Schema(l), SchemaOrBool::Schema(r)) => {
                    SchemaOrBool::Schema(importer.merge_schemas(l, r, context)?)
                }
                // Boolean schema vs real schema: prefer the real schema.
                (SchemaOrBool::Schema(s), SchemaOrBool::Bool(_))
                | (SchemaOrBool::Bool(_), SchemaOrBool::Schema(s)) => SchemaOrBool::Schema(s),
                // Both boolean: keep default.
                (SchemaOrBool::Bool(_), SchemaOrBool::Bool(_)) => SchemaOrBool::default(),
            };
            left.insert(key, merged);
        } else {
            left.insert(key, value);
        }
    }
    Ok(left)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json_test_source(spec: &str) -> OpenApiSource {
        OpenApiSource::new(SourceFormat::Json, spec.to_owned())
    }

    #[test]
    fn imports_minimal_openapi_document() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {
    "/widgets/{widget_id}": {
      "get": {
        "operationId": "get_widget",
        "parameters": [
          {
            "name": "widget_id",
            "in": "path",
            "required": true,
            "schema": { "type": "string" }
          }
        ],
        "responses": {
          "200": {
            "description": "ok",
            "content": {
              "application/json": {
                "schema": { "$ref": "#/components/schemas/Widget" }
              }
            }
          }
        }
      }
    }
  },
  "components": {
    "schemas": {
      "Widget": {
        "type": "object",
        "required": ["id"],
        "properties": {
          "status": {
            "$ref": "#/components/schemas/WidgetStatus"
          },
          "id": { "type": "string" },
          "count": { "anyOf": [{ "type": "integer" }, { "type": "null" }] },
          "labels": {
            "type": "object",
            "additionalProperties": { "type": "string" }
          },
          "metadata": {
            "type": "object",
            "additionalProperties": true
          }
        }
      },
      "WidgetStatus": {
        "type": "string",
        "enum": ["READY", "PAUSED"]
      }
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid test spec");
        let result = OpenApiImporter::new(
            document,
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect("should import successfully");
        let ir = result.ir;

        assert_eq!(ir.models.len(), 2);
        assert_eq!(ir.operations.len(), 1);
        assert_eq!(ir.operations[0].name, "get_widget");
        assert!(ir.models.iter().any(|model| model.name == "Widget"));
        assert!(ir.models.iter().any(|model| model.name == "WidgetStatus"));
        let widget = ir
            .models
            .iter()
            .find(|model| model.name == "Widget")
            .expect("widget model");
        assert!(
            widget
                .fields
                .iter()
                .find(|field| field.name == "count")
                .expect("count field")
                .nullable
        );
        assert!(matches!(
            widget
                .fields
                .iter()
                .find(|field| field.name == "metadata")
                .expect("metadata field")
                .type_ref,
            TypeRef::Map { .. }
        ));
        assert_eq!(
            widget
                .fields
                .iter()
                .find(|field| field.name == "status")
                .expect("status field")
                .type_ref,
            TypeRef::named("WidgetStatus")
        );
    }

    #[test]
    fn supports_parameter_refs() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {
    "/key/{PK}": {
      "delete": {
        "operationId": "delete_key",
        "parameters": [
          { "$ref": "#/components/parameters/PK" }
        ],
        "responses": {
          "204": { "description": "deleted" }
        }
      }
    }
  },
  "components": {
    "parameters": {
      "PK": {
        "name": "PK",
        "in": "path",
        "required": true,
        "schema": { "type": "string" }
      }
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid test spec");
        let result = OpenApiImporter::new(
            document,
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect("parameter refs should be supported");

        assert_eq!(result.ir.operations.len(), 1);
        let operation = &result.ir.operations[0];
        assert_eq!(operation.params.len(), 1);
        let param = &operation.params[0];
        assert_eq!(param.name, "PK");
        assert_eq!(param.location, ParameterLocation::Path);
        assert!(param.required);
        assert_eq!(param.type_ref, TypeRef::primitive("string"));
    }

    #[test]
    fn supports_swagger_root_parameter_refs_with_type() {
        let spec = r##"
{
  "swagger": "2.0",
  "paths": {
    "/widgets/{id}": {
      "get": {
        "operationId": "get_widget",
        "parameters": [
          { "$ref": "#/parameters/ApiVersionParameter" },
          { "$ref": "#/parameters/IdParameter" }
        ],
        "responses": {
          "200": { "description": "ok" }
        }
      }
    }
  },
  "parameters": {
    "ApiVersionParameter": {
      "name": "api-version",
      "in": "query",
      "required": true,
      "type": "string"
    },
    "IdParameter": {
      "name": "id",
      "in": "path",
      "required": true,
      "type": "integer",
      "format": "int64"
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid test spec");
        let result = OpenApiImporter::new(
            document,
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect("swagger root parameter refs should be supported");

        let operation = result
            .ir
            .operations
            .iter()
            .find(|operation| operation.name == "get_widget")
            .expect("operation should exist");
        assert_eq!(operation.params.len(), 2);
        assert_eq!(operation.params[0].name, "api-version");
        assert_eq!(operation.params[0].location, ParameterLocation::Query);
        assert_eq!(operation.params[0].type_ref, TypeRef::primitive("string"));
        assert_eq!(operation.params[1].name, "id");
        assert_eq!(operation.params[1].location, ParameterLocation::Path);
        assert_eq!(operation.params[1].type_ref, TypeRef::primitive("integer"));
    }

    #[test]
    fn supports_references_into_parameter_schemas() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {},
  "components": {
    "schemas": {
      "Widget": {
        "type": "object",
        "properties": {
          "companyId": {
            "$ref": "#/components/parameters/companyId/schema"
          }
        }
      }
    },
    "parameters": {
      "companyId": {
        "name": "companyId",
        "in": "path",
        "required": true,
        "schema": {
          "type": "string"
        }
      }
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid spec");
        let result = OpenApiImporter::new(
            document,
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect("schema refs into reusable parameters should resolve");

        let widget = result
            .ir
            .models
            .iter()
            .find(|model| model.name == "Widget")
            .expect("Widget model should exist");
        assert_eq!(widget.fields[0].name, "companyId");
        assert_eq!(widget.fields[0].type_ref, TypeRef::primitive("string"));
    }

    #[test]
    fn preserves_external_file_references_as_named_types() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {},
  "components": {
    "schemas": {
      "Route": {
        "type": "object",
        "properties": {
          "subnet": {
            "$ref": "./virtualNetwork.json#/definitions/Subnet"
          }
        }
      }
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid spec");
        let result = OpenApiImporter::new(
            document,
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect("external file refs should remain importable as named types");

        let route = result
            .ir
            .models
            .iter()
            .find(|model| model.name == "Route")
            .expect("Route model should exist");
        assert_eq!(route.fields[0].name, "subnet");
        assert_eq!(route.fields[0].type_ref, TypeRef::named("Subnet"));
    }

    #[test]
    fn normalizes_swagger_body_parameters_into_request_bodies() {
        let spec = r##"
{
  "swagger": "2.0",
  "consumes": ["application/json"],
  "paths": {
    "/widgets/{id}": {
      "patch": {
        "operationId": "patch_widget",
        "parameters": [
          {
            "name": "id",
            "in": "path",
            "required": true,
            "type": "string"
          },
          {
            "name": "widget",
            "in": "body",
            "required": true,
            "description": "Widget update payload.",
            "schema": {
              "type": "object",
              "properties": {
                "name": { "type": "string" }
              }
            }
          }
        ],
        "responses": {
          "200": { "description": "ok" }
        }
      }
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid test spec");
        let result = OpenApiImporter::new(
            document,
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect("swagger body parameters should become request bodies");

        let operation = result
            .ir
            .operations
            .iter()
            .find(|operation| operation.name == "patch_widget")
            .expect("operation should exist");
        assert_eq!(operation.params.len(), 1);
        assert_eq!(operation.params[0].name, "id");

        let request_body = operation.request_body.as_ref().expect("request body");
        assert!(request_body.required);
        assert_eq!(request_body.media_type, "application/json");
        assert_eq!(
            request_body.attributes.get("description"),
            Some(&Value::String("Widget update payload.".into()))
        );
        assert!(matches!(request_body.type_ref, Some(TypeRef::Named { .. })));
    }

    #[test]
    fn supports_request_body_refs() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {
    "/events": {
      "post": {
        "operationId": "create_event",
        "requestBody": {
          "$ref": "#/components/requestBodies/EventRequest"
        },
        "responses": {
          "200": { "description": "ok" }
        }
      }
    }
  },
  "components": {
    "requestBodies": {
      "EventRequest": {
        "$ref": "#/components/requestBodies/BaseEventRequest"
      },
      "BaseEventRequest": {
        "required": true,
        "content": {
          "application/json": {
            "schema": { "type": "string" }
          }
        }
      }
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid test spec");
        let result = OpenApiImporter::new(
            document,
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect("request body refs should be supported");

        let operation = result
            .ir
            .operations
            .iter()
            .find(|operation| operation.name == "create_event")
            .expect("operation should exist");
        let request_body = operation.request_body.as_ref().expect("request body");
        assert!(request_body.required);
        assert_eq!(request_body.media_type, "application/json");
        assert_eq!(request_body.type_ref, Some(TypeRef::primitive("string")));
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn defaults_empty_request_body_content_to_untyped_octet_stream() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {
    "/events": {
      "post": {
        "operationId": "create_event",
        "requestBody": {
          "required": true,
          "content": {}
        },
        "responses": {
          "200": { "description": "ok" }
        }
      }
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid test spec");
        let result = OpenApiImporter::new(
            document,
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect("empty request body content should be normalized");

        let operation = result
            .ir
            .operations
            .iter()
            .find(|operation| operation.name == "create_event")
            .expect("operation should exist");
        let request_body = operation.request_body.as_ref().expect("request body");
        assert!(request_body.required);
        assert_eq!(request_body.media_type, "application/octet-stream");
        assert_eq!(request_body.type_ref, None);
        assert_eq!(result.warnings.len(), 1);
        assert!(matches!(
            result.warnings[0].kind,
            DiagnosticKind::EmptyRequestBodyContent
        ));
        assert_eq!(
            result.warnings[0].pointer.as_deref(),
            Some("#/paths/~1events/post/requestBody/content")
        );
    }

    #[test]
    fn supports_const_scalar_fields() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {},
  "components": {
    "schemas": {
      "PatchOp": {
        "type": "object",
        "properties": {
          "op": {
            "type": "string",
            "const": "replace"
          }
        }
      }
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid test spec");
        let result = OpenApiImporter::new(
            document,
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect("const should be supported");
        let patch_op = result
            .ir
            .models
            .iter()
            .find(|model| model.name == "PatchOp")
            .expect("PatchOp model");
        assert!(
            patch_op
                .fields
                .iter()
                .any(|field| field.name == "op" && field.type_ref == TypeRef::primitive("string"))
        );
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn supports_type_array_with_nullability() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {},
  "components": {
    "schemas": {
      "Widget": {
        "type": "object",
        "properties": {
          "name": {
            "type": ["string", "null"]
          }
        }
      }
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid test spec");
        let result = OpenApiImporter::new(
            document,
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect("type arrays with null should be supported");
        let widget = result
            .ir
            .models
            .iter()
            .find(|model| model.name == "Widget")
            .expect("Widget model");
        let name = widget
            .fields
            .iter()
            .find(|field| field.name == "name")
            .expect("name field");
        assert_eq!(name.type_ref, TypeRef::primitive("string"));
        assert!(name.nullable);
    }

    #[test]
    fn falls_back_when_operation_id_is_empty() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {
    "/widgets": {
      "get": {
        "operationId": "",
        "responses": {
          "200": { "description": "ok" }
        }
      }
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid test spec");
        let result = OpenApiImporter::new(
            document,
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect("empty operation ids should fall back");
        let operation = &result.ir.operations[0];
        assert_eq!(operation.name, "get_widgets");
    }

    #[test]
    fn supports_implicit_enum_and_items_schema_shapes() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {},
  "components": {
    "schemas": {
      "Widget": {
        "type": "object",
        "properties": {
          "status": {
            "enum": ["ready", "pending"]
          },
          "children": {
            "items": {
              "type": "string"
            }
          },
          "withTrial": {
            "format": "boolean"
          }
        }
      }
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid test spec");
        let result = OpenApiImporter::new(
            document,
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect("implicit enum/items/format schema shapes should be supported");
        let widget = result
            .ir
            .models
            .iter()
            .find(|model| model.name == "Widget")
            .expect("Widget model");
        let status = widget
            .fields
            .iter()
            .find(|field| field.name == "status")
            .expect("status field");
        assert_eq!(status.type_ref, TypeRef::primitive("string"));

        let children = widget
            .fields
            .iter()
            .find(|field| field.name == "children")
            .expect("children field");
        assert_eq!(
            children.type_ref,
            TypeRef::array(TypeRef::primitive("string"))
        );

        let with_trial = widget
            .fields
            .iter()
            .find(|field| field.name == "withTrial")
            .expect("withTrial field");
        assert_eq!(with_trial.type_ref, TypeRef::primitive("boolean"));
    }

    #[test]
    fn supports_object_schemas_with_validation_only_any_of() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {},
  "components": {
    "schemas": {
      "PatchGist": {
        "type": "object",
        "properties": {
          "description": { "type": "string" },
          "files": { "type": "object" }
        },
        "anyOf": [
          { "required": ["description"] },
          { "required": ["files"] }
        ],
        "nullable": true
      }
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid test spec");
        let result = OpenApiImporter::new(
            document,
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect("object schemas with validation-only anyOf should be supported");
        let patch_gist = result
            .ir
            .models
            .iter()
            .find(|model| model.name == "PatchGist")
            .expect("PatchGist model");
        let field_names = patch_gist
            .fields
            .iter()
            .map(|field| field.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(field_names, vec!["description", "files"]);
    }

    #[test]
    fn preserves_schema_property_order() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {},
  "components": {
    "schemas": {
      "Widget": {
        "type": "object",
        "properties": {
          "zebra": { "type": "string" },
          "alpha": { "type": "string" },
          "middle": { "type": "string" }
        }
      }
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid test spec");
        let result = OpenApiImporter::new(
            document,
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect("property order should be preserved");
        let widget = result
            .ir
            .models
            .iter()
            .find(|model| model.name == "Widget")
            .expect("Widget model");
        let field_names = widget
            .fields
            .iter()
            .map(|field| field.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(field_names, vec!["zebra", "alpha", "middle"]);
    }

    #[test]
    fn supports_metadata_only_property_schema_as_any() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {},
  "components": {
    "schemas": {
      "ErrorDetail": {
        "type": "object",
        "properties": {
          "value": {
            "description": "The value at the given location"
          }
        }
      }
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid test spec");
        let result = OpenApiImporter::new(
            document,
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect("metadata-only schema should be treated as any");
        let error_detail = result
            .ir
            .models
            .iter()
            .find(|model| model.name == "ErrorDetail")
            .expect("ErrorDetail model");
        let value = error_detail
            .fields
            .iter()
            .find(|field| field.name == "value")
            .expect("value field");
        assert_eq!(value.type_ref, TypeRef::primitive("any"));
    }

    #[test]
    fn supports_discriminator_on_unions() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {},
  "components": {
    "schemas": {
      "AddOperation": {
        "type": "object",
        "properties": {
          "op": {
            "type": "string",
            "const": "add"
          }
        }
      },
      "RemoveOperation": {
        "type": "object",
        "properties": {
          "op": {
            "type": "string",
            "const": "remove"
          }
        }
      },
      "PatchSchema": {
        "type": "object",
        "properties": {
          "patches": {
            "type": "array",
            "items": {
              "oneOf": [
                { "$ref": "#/components/schemas/AddOperation" },
                { "$ref": "#/components/schemas/RemoveOperation" }
              ],
              "discriminator": {
                "propertyName": "op",
                "mapping": {
                  "add": "#/components/schemas/AddOperation",
                  "remove": "#/components/schemas/RemoveOperation"
                }
              }
            }
          }
        }
      }
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid test spec");
        let result = OpenApiImporter::new(
            document,
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect("discriminator unions should be supported");
        let patch_schema = result
            .ir
            .models
            .iter()
            .find(|model| model.name == "PatchSchema")
            .expect("PatchSchema model");
        let patches = patch_schema
            .fields
            .iter()
            .find(|field| field.name == "patches")
            .expect("patches field");
        assert!(matches!(
            &patches.type_ref,
            TypeRef::Array { item }
                if matches!(
                    item.as_ref(),
                    TypeRef::Union { variants }
                        if variants == &vec![
                            TypeRef::named("AddOperation"),
                            TypeRef::named("RemoveOperation")
                        ]
                )
        ));
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn supports_all_of_object_composition() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {},
  "components": {
    "schemas": {
      "Cursor": {
        "type": "object",
        "properties": {
          "cursor": { "type": "string" }
        },
        "required": ["cursor"]
      },
      "PatchSchema": {
        "allOf": [
          { "$ref": "#/components/schemas/Cursor" },
          {
            "type": "object",
            "properties": {
              "items": {
                "type": "array",
                "items": { "type": "string" }
              }
            },
            "required": ["items"]
          }
        ]
      },
      "BaseId": { "type": "string" },
      "WrappedId": {
        "allOf": [
          { "$ref": "#/components/schemas/BaseId" },
          { "description": "Identifier wrapper" }
        ]
      },
      "Status": {
        "type": "string",
        "enum": ["ready", "pending", "failed"]
      },
      "RetryableStatus": {
        "allOf": [
          { "$ref": "#/components/schemas/Status" },
          { "enum": ["pending", "failed"] }
        ]
      },
      "TitledCursor": {
        "allOf": [
          {
            "$ref": "#/components/schemas/Cursor",
            "title": "Cursor Base"
          },
          {
            "type": "object",
            "title": "Cursor Overlay",
            "properties": {
              "nextCursor": { "type": "string" }
            }
          }
        ]
      },
      "Wrapper": {
        "type": "object",
        "properties": {
          "cursorRef": {
            "allOf": [
              { "$ref": "#/components/schemas/Cursor" },
              { "description": "Keep the named component reference" }
            ]
          }
        }
      }
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid test spec");
        let result = OpenApiImporter::new(
            document,
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect("allOf should be supported");

        let patch_schema = result
            .ir
            .models
            .iter()
            .find(|model| model.name == "PatchSchema")
            .expect("PatchSchema model");
        let field_names = patch_schema
            .fields
            .iter()
            .map(|field| field.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(field_names, vec!["cursor", "items"]);

        let titled_cursor = result
            .ir
            .models
            .iter()
            .find(|model| model.name == "TitledCursor")
            .expect("TitledCursor model");
        let titled_cursor_fields = titled_cursor
            .fields
            .iter()
            .map(|field| field.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(titled_cursor_fields, vec!["cursor", "nextCursor"]);

        let retryable_status = result
            .ir
            .models
            .iter()
            .find(|model| model.name == "RetryableStatus")
            .expect("RetryableStatus model");
        assert_eq!(
            retryable_status.attributes.get("enum_values"),
            Some(&Value::Array(vec![
                Value::String("pending".into()),
                Value::String("failed".into())
            ]))
        );
        assert!(
            patch_schema
                .fields
                .iter()
                .find(|field| field.name == "cursor")
                .map(|field| !field.optional)
                .unwrap_or(false)
        );
        assert!(
            patch_schema
                .fields
                .iter()
                .find(|field| field.name == "items")
                .map(|field| !field.optional)
                .unwrap_or(false)
        );

        let wrapped_id = result
            .ir
            .models
            .iter()
            .find(|model| model.name == "WrappedId")
            .expect("WrappedId model");
        assert_eq!(
            wrapped_id.attributes.get("alias_type_ref"),
            Some(&json!(TypeRef::primitive("string")))
        );
        let wrapper = result
            .ir
            .models
            .iter()
            .find(|model| model.name == "Wrapper")
            .expect("Wrapper model");
        assert_eq!(
            wrapper
                .fields
                .iter()
                .find(|field| field.name == "cursorRef")
                .map(|field| &field.type_ref),
            Some(&TypeRef::named("Cursor"))
        );
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn errors_on_recursive_all_of_reference_cycles() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {},
  "components": {
    "schemas": {
      "Node": {
        "allOf": [
          { "$ref": "#/components/schemas/Node" }
        ]
      }
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid test spec");
        let error = OpenApiImporter::new(
            document,
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect_err("recursive allOf cycles should fail cleanly");

        let rendered = format!("{error:#}");
        assert!(
            rendered.contains("recursive reference cycle"),
            "unexpected error: {rendered}"
        );
        assert!(rendered.contains("#/components/schemas/Node"));
    }

    #[test]
    fn errors_on_unhandled_elements_by_default_and_warns_when_ignored() {
        // `not` is now silently ignored (mapped to `any`). Verify a genuinely
        // unsupported-but-declared keyword (`if`) still triggers the unhandled path.
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {},
  "components": {
    "schemas": {
      "PatchSchema": {
        "if": {
          "properties": { "foo": { "type": "string" } }
        }
      }
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid test spec");
        let strict_error = OpenApiImporter::new(
            document.clone(),
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect_err("strict mode should fail");
        assert!(
            strict_error
                .to_string()
                .contains("`if` is not supported yet")
        );

        let warning_result = OpenApiImporter::new(
            document,
            json_test_source(spec),
            LoadOpenApiOptions {
                ignore_unhandled: true,
                ..Default::default()
            },
        )
        .build_ir()
        .expect("ignore mode should succeed");
        assert!(
            warning_result
                .warnings
                .iter()
                .any(|warning| matches!(&warning.kind, DiagnosticKind::UnsupportedSchemaKeyword { keyword } if keyword == "if"))
        );

        // Verify `not` is silently ignored (no error, no warning).
        let not_spec = r##"
{
  "openapi": "3.1.0",
  "paths": {},
  "components": {
    "schemas": {
      "NotSchema": {
        "not": { "type": "object" }
      }
    }
  }
}
"##;
        let not_document: OpenApiDocument =
            serde_json::from_str(not_spec).expect("valid test spec");
        let not_result = OpenApiImporter::new(
            not_document,
            json_test_source(not_spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect("`not` keyword should be silently ignored");
        assert!(
            not_result.warnings.is_empty(),
            "`not` should produce no warnings"
        );
    }

    #[test]
    fn errors_on_unknown_schema_keywords() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {},
  "components": {
    "schemas": {
      "PatchSchema": {
        "type": "string",
        "frobnicate": true
      }
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid test spec");
        let error = OpenApiImporter::new(
            document,
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect_err("unknown keyword should fail");
        assert!(
            error
                .to_string()
                .contains("unknown schema keyword `frobnicate`")
        );
    }

    #[test]
    fn ignores_known_non_codegen_schema_keywords() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {},
  "components": {
    "schemas": {
      "PatchSchema": {
        "type": "string",
        "description": "some text",
        "default": "value",
        "minLength": 1,
        "contentEncoding": "base64",
        "externalDocs": {
          "description": "More details",
          "url": "https://example.com/schema-docs"
        },
        "xml": {
          "name": "patchSchema"
        }
      }
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid test spec");
        let result = OpenApiImporter::new(
            document,
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect("known ignored keywords should not fail");
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn json_parse_errors_include_schema_path_and_source_context() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {},
  "components": {
    "schemas": {
      "Broken": {
        "type": "object",
        "title": ["not", "a", "string"]
      }
    }
  }
}
"##;

        let error = parse_json_openapi_document(Path::new("broken.json"), spec)
            .expect_err("invalid schema shape should fail during deserialization");
        let message = error.to_string();
        assert!(message.contains("failed to parse JSON OpenAPI document `broken.json`"));
        assert!(message.contains("schema mismatch at `components.schemas.Broken.title`"));
        assert!(message.contains("invalid type"));
        assert!(message.contains("source:         \"title\": [\"not\", \"a\", \"string\"]"));
        assert!(message.contains("note: this usually means"));
    }

    #[test]
    fn yaml_loader_ignores_tab_only_blank_lines_in_block_scalars() {
        let spec = r##"
openapi: 3.1.0
paths: {}
components:
  schemas:
    AdditionalDataAirline:
      type: object
      properties:
        airline.leg.date_of_travel:
          description: |-
            	
            Date and time of travel in ISO 8601 format.
          type: string
"##;

        let loaded = parse_yaml_openapi_document(Path::new("broken.yaml"), spec)
            .expect("tab-only blank lines should be normalized before YAML parsing");
        let result = OpenApiImporter::new(
            loaded.document,
            loaded.source,
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect("normalized YAML should import");

        let model = result
            .ir
            .models
            .iter()
            .find(|model| model.name == "AdditionalDataAirline")
            .expect("model should exist");
        assert!(
            model
                .fields
                .iter()
                .any(|field| field.name == "airline.leg.date_of_travel")
        );
    }

    #[test]
    fn preserves_content_encoding_metadata_in_ir_attributes() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {
    "/widgets": {
      "post": {
        "operationId": "create_widget",
        "parameters": [
          {
            "name": "token",
            "in": "query",
            "required": true,
            "schema": {
              "type": "string",
              "contentEncoding": "base64"
            }
          }
        ],
        "requestBody": {
          "required": true,
          "content": {
            "application/json": {
              "schema": {
                "type": "string",
                "contentEncoding": "base64",
                "contentMediaType": "application/octet-stream"
              }
            }
          }
        },
        "responses": {
          "200": {
            "description": "ok",
            "content": {
              "application/json": {
                "schema": {
                  "$ref": "#/components/schemas/EncodedValue"
                }
              }
            }
          }
        }
      }
    }
  },
  "components": {
    "schemas": {
      "EncodedValue": {
        "type": "object",
        "properties": {
          "payload": {
            "type": "string",
            "contentEncoding": "base64"
          }
        }
      }
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid test spec");
        let result = OpenApiImporter::new(
            document,
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect("content encoding metadata should be preserved");

        let operation = result
            .ir
            .operations
            .iter()
            .find(|operation| operation.name == "create_widget")
            .expect("operation should exist");
        assert_eq!(
            operation.params[0]
                .attributes
                .get("content_encoding")
                .and_then(Value::as_str),
            Some("base64")
        );
        assert_eq!(
            operation
                .request_body
                .as_ref()
                .and_then(|request_body| request_body.attributes.get("content_media_type"))
                .and_then(Value::as_str),
            Some("application/octet-stream")
        );
        let response = operation
            .responses
            .iter()
            .find(|response| response.status == "200")
            .expect("response should exist");
        assert_eq!(
            response.type_ref.as_ref(),
            Some(&TypeRef::named("EncodedValue"))
        );
        let model = result
            .ir
            .models
            .iter()
            .find(|model| model.name == "EncodedValue")
            .expect("model should exist");
        assert_eq!(
            model.fields[0]
                .attributes
                .get("content_encoding")
                .and_then(Value::as_str),
            Some("base64")
        );
    }

    #[test]
    fn supports_swagger_form_data_parameters() {
        let spec = r##"
{
  "swagger": "2.0",
  "consumes": ["application/x-www-form-urlencoded"],
  "paths": {
    "/widgets": {
      "post": {
        "operationId": "create_widget",
        "parameters": [
          { "$ref": "#/parameters/form_name" }
        ],
        "responses": {
          "200": {
            "description": "ok"
          }
        }
      }
    }
  },
  "parameters": {
    "form_name": {
      "name": "name",
      "in": "formData",
      "description": "Widget name",
      "required": true,
      "type": "string"
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid swagger spec");
        let result = OpenApiImporter::new(
            document,
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect("formData parameters should normalize into a request body");
        let operation = result
            .ir
            .operations
            .iter()
            .find(|operation| operation.name == "create_widget")
            .expect("operation should exist");
        assert!(operation.params.is_empty());
        let request_body = operation
            .request_body
            .as_ref()
            .expect("formData should create a request body");
        assert_eq!(request_body.media_type, "application/x-www-form-urlencoded");
        assert_eq!(
            request_body.type_ref.as_ref(),
            Some(&TypeRef::named("CreateWidgetRequest"))
        );
        let body_model = result
            .ir
            .models
            .iter()
            .find(|model| model.name == "CreateWidgetRequest")
            .expect("inline form body model should exist");
        assert_eq!(body_model.fields[0].name, "name");
        assert!(!body_model.fields[0].optional);
        assert_eq!(
            body_model.fields[0]
                .attributes
                .get("description")
                .and_then(Value::as_str),
            Some("Widget name")
        );
    }

    #[test]
    fn supports_path_local_parameter_references() {
        let spec = r##"
{
  "swagger": "2.0",
  "definitions": {
    "Widget": {
      "type": "object",
      "properties": {
        "id": { "type": "string" }
      }
    }
  },
  "paths": {
    "/widgets/{id}": {
      "post": {
        "operationId": "get_widget",
        "parameters": [
          {
            "name": "id",
            "in": "path",
            "required": true,
            "type": "string"
          }
        ],
        "responses": {
          "200": {
            "description": "ok",
            "schema": {
              "$ref": "#/definitions/Widget"
            }
          }
        }
      }
    },
    "/widget-ids": {
      "get": {
        "operationId": "list_widget_ids",
        "parameters": [
          {
            "$ref": "#/paths/~1widgets~1%7Bid%7D/post/parameters/0"
          }
        ],
        "responses": {
          "200": {
            "description": "ok"
          }
        }
      }
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid swagger spec");
        let result = OpenApiImporter::new(
            document,
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect("path-local parameter refs should resolve");
        let operation = result
            .ir
            .operations
            .iter()
            .find(|operation| operation.name == "list_widget_ids")
            .expect("operation should exist");
        assert_eq!(operation.params[0].name, "id");
        assert_eq!(operation.params[0].location, ParameterLocation::Path);
        assert_eq!(
            result
                .ir
                .models
                .iter()
                .find(|model| model.name == "Widget")
                .map(|model| model.name.as_str()),
            Some("Widget")
        );
    }

    #[test]
    fn de_duplicates_operation_names() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {
    "/widgets": {
      "get": {
        "operationId": "get_widgets",
        "responses": {
          "200": { "description": "ok" }
        }
      }
    },
    "/users": {
      "get": {
        "operationId": "get_widgets",
        "responses": {
          "200": { "description": "ok" }
        }
      }
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid spec");
        let result = OpenApiImporter::new(
            document,
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect("duplicate operation ids should be disambiguated");
        let names = result
            .ir
            .operations
            .iter()
            .map(|operation| operation.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["get_widgets", "get_widgets_2"]);
    }

    #[test]
    fn supports_numeric_all_of_type_widening() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {},
  "components": {
    "schemas": {
      "WidgetEvent": {
        "type": "object",
        "properties": {
          "payload": {
            "allOf": [
              {
                "type": "object",
                "properties": {
                  "count": { "type": "integer" }
                }
              },
              {
                "type": "object",
                "properties": {
                  "count": { "type": "number" }
                }
              }
            ]
          }
        }
      }
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid spec");
        let result = OpenApiImporter::new(
            document,
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect("numeric allOf overlays should merge");
        assert!(result.warnings.is_empty());
        let payload_model = result
            .ir
            .models
            .iter()
            .find(|model| model.name == "WidgetEventPayload")
            .expect("inline payload model should exist");
        assert_eq!(
            payload_model.fields[0].type_ref,
            TypeRef::primitive("number")
        );
    }

    #[test]
    fn supports_nested_schema_definitions_references() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {},
  "components": {
    "schemas": {
      "Transfer": {
        "type": "object",
        "definitions": {
          "money": {
            "type": "object",
            "properties": {
              "currency": { "type": "string" }
            }
          }
        },
        "properties": {
          "amount": {
            "$ref": "#/components/schemas/Transfer/definitions/money"
          },
          "currency": {
            "$ref": "#/components/schemas/Transfer/definitions/money/properties/currency"
          }
        }
      }
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid spec");
        let result = OpenApiImporter::new(
            document,
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect("nested schema definitions refs should resolve");
        let transfer = result
            .ir
            .models
            .iter()
            .find(|model| model.name == "Transfer")
            .expect("Transfer model should exist");
        assert_eq!(transfer.fields[0].name, "amount");
        let amount_type_name = match &transfer.fields[0].type_ref {
            TypeRef::Named { name } => name.clone(),
            other => panic!("expected named type for nested definition, got {other:?}"),
        };
        assert!(
            amount_type_name.starts_with("TransferAmount"),
            "nested definition should be materialized as a TransferAmount* inline model"
        );
        assert!(
            result
                .ir
                .models
                .iter()
                .any(|model| model.name == amount_type_name),
            "nested local definition model should be imported"
        );
        assert_eq!(transfer.fields[1].type_ref, TypeRef::primitive("string"));
    }

    #[test]
    fn supports_nullable_all_of_overlays_on_referenced_scalars() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {},
  "components": {
    "schemas": {
      "Transfer": {
        "type": "object",
        "definitions": {
          "money": {
            "type": "string"
          }
        }
      },
      "Bill": {
        "type": "object",
        "properties": {
          "currency": {
            "allOf": [
              { "$ref": "#/components/schemas/Transfer/definitions/money" },
              { "type": "null" }
            ]
          }
        }
      }
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid spec");
        let result = OpenApiImporter::new(
            document,
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect("nullable allOf overlay should merge");
        let bill = result
            .ir
            .models
            .iter()
            .find(|model| model.name == "Bill")
            .expect("Bill model should exist");
        assert_eq!(bill.fields[0].name, "currency");
        assert_eq!(bill.fields[0].type_ref, TypeRef::primitive("string"));
        assert!(bill.fields[0].nullable);
    }

    #[test]
    fn supports_recursive_local_object_references_without_unbounded_inline_models() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {},
  "components": {
    "schemas": {
      "PushOption": {
        "definitions": {
          "pushOptionProperty": {
            "type": "object",
            "properties": {
              "properties": {
                "type": "object",
                "additionalProperties": {
                  "$ref": "#/components/schemas/PushOption/definitions/pushOptionProperty"
                }
              }
            }
          }
        },
        "type": "object",
        "properties": {
          "properties": {
            "type": "object",
            "additionalProperties": {
              "$ref": "#/components/schemas/PushOption/definitions/pushOptionProperty"
            }
          }
        }
      }
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid spec");
        let result = OpenApiImporter::new(
            document,
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect("recursive local refs should not recurse forever");

        let push_option = result
            .ir
            .models
            .iter()
            .find(|model| model.name == "PushOption")
            .expect("PushOption model should exist");
        let properties_field = push_option
            .fields
            .iter()
            .find(|field| field.name == "properties")
            .expect("properties field should exist");
        assert!(matches!(properties_field.type_ref, TypeRef::Map { .. }));

        let inline_models = result
            .ir
            .models
            .iter()
            .filter(|model| model.name.contains("Properties"))
            .collect::<Vec<_>>();
        assert!(
            inline_models.len() <= 2,
            "recursive local refs should reuse an inline model instead of generating an unbounded chain"
        );
    }

    #[test]
    fn supports_collection_format_metadata() {
        let spec = r##"
{
  "swagger": "2.0",
  "paths": {
    "/widgets": {
      "get": {
        "operationId": "list_widgets",
        "parameters": [
          {
            "name": "categories",
            "in": "query",
            "type": "array",
            "collectionFormat": "csv",
            "items": {
              "type": "string",
              "collectionFormat": "csv"
            }
          }
        ],
        "responses": {
          "200": {
            "description": "ok"
          }
        }
      }
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid spec");
        let result = OpenApiImporter::new(
            document,
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect("collectionFormat should be accepted");
        let operation = result
            .ir
            .operations
            .iter()
            .find(|operation| operation.name == "list_widgets")
            .expect("operation should exist");
        assert_eq!(
            operation.params[0]
                .attributes
                .get("collection_format")
                .and_then(Value::as_str),
            Some("csv")
        );
    }

    #[test]
    fn supports_all_of_with_multiple_discriminators() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {},
  "components": {
    "schemas": {
      "Base": {
        "type": "object",
        "discriminator": {
          "propertyName": "serviceType"
        },
        "properties": {
          "serviceType": { "type": "string" }
        }
      },
      "Derived": {
        "allOf": [
          { "$ref": "#/components/schemas/Base" },
          {
            "type": "object",
            "discriminator": {
              "propertyName": "credentialType"
            },
            "properties": {
              "credentialType": { "type": "string" }
            }
          }
        ]
      }
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid spec");
        let result = OpenApiImporter::new(
            document,
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect("allOf discriminator metadata should not fail");
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn empty_property_names_fail_cleanly_or_warn_when_ignored() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {},
  "components": {
    "schemas": {
      "Broken": {
        "type": "object",
        "properties": {
          "": { "type": "string" }
        }
      }
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid spec");
        let error = OpenApiImporter::new(
            document.clone(),
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect_err("empty property names should fail by default");
        assert!(error.to_string().contains("property #1 has an empty name"));

        let result = OpenApiImporter::new(
            document,
            json_test_source(spec),
            LoadOpenApiOptions {
                ignore_unhandled: true,
                emit_timings: false,
            },
        )
        .build_ir()
        .expect("empty property names should be synthesized when warnings are allowed");
        let broken = result
            .ir
            .models
            .iter()
            .find(|model| model.name == "Broken")
            .expect("Broken model should exist");
        assert_eq!(broken.fields[0].name, "unnamed_field_1");
    }

    #[test]
    fn supports_ref_to_components_responses() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {
    "/widgets": {
      "get": {
        "operationId": "list_widgets",
        "parameters": [],
        "responses": {
          "200": {
            "description": "ok",
            "content": {
              "application/json": {
                "schema": { "$ref": "#/components/responses/WidgetList/content/application~1json/schema" }
              }
            }
          }
        }
      }
    }
  },
  "components": {
    "responses": {
      "WidgetList": {
        "description": "A list of widgets",
        "content": {
          "application/json": {
            "schema": {
              "type": "array",
              "items": { "type": "string" }
            }
          }
        }
      }
    }
  }
}
"##;
        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid spec");
        let result = OpenApiImporter::new(
            document,
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect("$ref to components/responses should succeed");
        let op = result
            .ir
            .operations
            .iter()
            .find(|o| o.name == "list_widgets")
            .expect("op should exist");
        let response = op.responses.first().expect("should have a response");
        // The $ref resolves to an array type; type_ref should be Some (not None).
        assert!(
            response.type_ref.is_some(),
            "response type_ref should be resolved, got: {response:?}"
        );
    }

    #[test]
    fn supports_ref_to_path_response_schema() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {
    "/widgets": {
      "get": {
        "operationId": "list_widgets",
        "parameters": [],
        "responses": {
          "200": {
            "description": "ok",
            "content": {
              "application/json": {
                "schema": { "type": "array", "items": { "type": "string" } }
              }
            }
          }
        }
      },
      "post": {
        "operationId": "create_widget",
        "parameters": [],
        "responses": {
          "201": {
            "description": "created",
            "content": {
              "application/json": {
                "schema": { "$ref": "#/paths/~1widgets/get/responses/200/content/application~1json/schema" }
              }
            }
          }
        }
      }
    }
  }
}
"##;
        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid spec");
        let result = OpenApiImporter::new(
            document,
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect("$ref to path response schema should succeed");
        let op = result
            .ir
            .operations
            .iter()
            .find(|o| o.name == "create_widget")
            .expect("op should exist");
        let response = op.responses.first().expect("should have a response");
        // The $ref resolves to an array-of-string type; type_ref should be Some.
        assert!(
            response.type_ref.is_some(),
            "response type_ref should be resolved, got: {response:?}"
        );
    }

    #[test]
    fn supports_content_based_parameters() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {
    "/widgets": {
      "get": {
        "operationId": "list_widgets",
        "parameters": [
          {
            "name": "filter",
            "in": "query",
            "content": {
              "application/json": {
                "schema": { "type": "object", "properties": { "name": { "type": "string" } } }
              }
            }
          }
        ],
        "responses": { "200": { "description": "ok" } }
      }
    }
  }
}
"##;
        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid spec");
        let result = OpenApiImporter::new(
            document,
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect("content-based parameter should succeed");
        let op = result
            .ir
            .operations
            .iter()
            .find(|o| o.name == "list_widgets")
            .expect("op should exist");
        assert_eq!(op.params.len(), 1);
        assert_eq!(op.params[0].name, "filter");
    }

    #[test]
    fn supports_format_string_as_type_inference() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {},
  "components": {
    "schemas": {
      "Widget": {
        "type": "object",
        "properties": {
          "name": { "format": "string", "description": "The widget name" },
          "score": { "format": "float" }
        }
      }
    }
  }
}
"##;
        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid spec");
        let result = OpenApiImporter::new(
            document,
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect("format:string schema shape should succeed");
        let model = result
            .ir
            .models
            .iter()
            .find(|m| m.name == "Widget")
            .expect("model should exist");
        let name_field = model
            .fields
            .iter()
            .find(|f| f.name == "name")
            .expect("name field should exist");
        assert!(matches!(&name_field.type_ref, t if format!("{t:?}").contains("string")));
        let score_field = model
            .fields
            .iter()
            .find(|f| f.name == "score")
            .expect("score field should exist");
        assert!(matches!(&score_field.type_ref, t if format!("{t:?}").contains("number")));
    }
}
