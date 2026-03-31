use arvalez_ir::CoreIr;

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