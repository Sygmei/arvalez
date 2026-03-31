use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use arvalez_ir::{CoreIr, HttpMethod, Operation, ParameterLocation, TypeRef};
pub use arvalez_target_core::GeneratedFile;
pub use arvalez_target_core::write_files as write_typescript_package;
use arvalez_target_core::{
    ClientLayout, indent_block, load_templates, sorted_models, sorted_operations,
};
use serde::Serialize;
use serde_json::{Value, from_value};
use tera::{Context as TeraContext, Tera};

const TEMPLATE_PACKAGE_JSON: &str = "package/package.json.tera";
const TEMPLATE_TSCONFIG: &str = "package/tsconfig.json.tera";
const TEMPLATE_README: &str = "package/README.md.tera";
const TEMPLATE_MODELS: &str = "package/models.ts.tera";
const TEMPLATE_CLIENT: &str = "package/client.ts.tera";
const TEMPLATE_INDEX: &str = "package/index.ts.tera";
const TEMPLATE_MODEL_INTERFACE: &str = "partials/model_interface.ts.tera";
const TEMPLATE_CLIENT_METHOD: &str = "partials/client_method.ts.tera";
const TEMPLATE_TAG_GROUP: &str = "partials/tag_group.ts.tera";

const BUILTIN_TEMPLATES: &[(&str, &str)] = &[
    (
        TEMPLATE_PACKAGE_JSON,
        include_str!("../templates/package/package.json.tera"),
    ),
    (
        TEMPLATE_TSCONFIG,
        include_str!("../templates/package/tsconfig.json.tera"),
    ),
    (
        TEMPLATE_README,
        include_str!("../templates/package/README.md.tera"),
    ),
    (
        TEMPLATE_MODELS,
        include_str!("../templates/package/models.ts.tera"),
    ),
    (
        TEMPLATE_CLIENT,
        include_str!("../templates/package/client.ts.tera"),
    ),
    (
        TEMPLATE_INDEX,
        include_str!("../templates/package/index.ts.tera"),
    ),
    (
        TEMPLATE_MODEL_INTERFACE,
        include_str!("../templates/partials/model_interface.ts.tera"),
    ),
    (
        TEMPLATE_CLIENT_METHOD,
        include_str!("../templates/partials/client_method.ts.tera"),
    ),
    (
        TEMPLATE_TAG_GROUP,
        include_str!("../templates/partials/tag_group.ts.tera"),
    ),
];

const OVERRIDABLE_TEMPLATES: &[&str] = &[
    TEMPLATE_PACKAGE_JSON,
    TEMPLATE_TSCONFIG,
    TEMPLATE_README,
    TEMPLATE_MODELS,
    TEMPLATE_CLIENT,
    TEMPLATE_INDEX,
    TEMPLATE_MODEL_INTERFACE,
    TEMPLATE_CLIENT_METHOD,
    TEMPLATE_TAG_GROUP,
];

#[derive(Debug, Clone)]
pub struct TypeScriptPackageConfig {
    pub package_name: String,
    pub version: String,
    pub template_dir: Option<PathBuf>,
    pub group_by_tag: bool,
}

impl TypeScriptPackageConfig {
    pub fn new(package_name: impl Into<String>) -> Self {
        Self {
            package_name: package_name.into(),
            version: "0.1.0".into(),
            template_dir: None,
            group_by_tag: false,
        }
    }

    pub fn with_version(mut self, version: impl Into<String>) -> Self {
        self.version = version.into();
        self
    }

    pub fn with_template_dir(mut self, template_dir: Option<PathBuf>) -> Self {
        self.template_dir = template_dir;
        self
    }

    pub fn with_group_by_tag(mut self, group_by_tag: bool) -> Self {
        self.group_by_tag = group_by_tag;
        self
    }
}

pub fn generate_typescript_package(
    ir: &CoreIr,
    config: &TypeScriptPackageConfig,
) -> Result<Vec<GeneratedFile>> {
    let tera = load_templates(config.template_dir.as_deref(), BUILTIN_TEMPLATES, OVERRIDABLE_TEMPLATES)?;
    let package_context = PackageTemplateContext::from_ir(ir, config, &tera)?;
    let mut template_context = TeraContext::new();
    template_context.insert("package", &package_context);

    Ok(vec![
        GeneratedFile {
            path: PathBuf::from("package.json"),
            contents: tera
                .render(TEMPLATE_PACKAGE_JSON, &template_context)
                .context("failed to render package.json template")?,
        },
        GeneratedFile {
            path: PathBuf::from("tsconfig.json"),
            contents: tera
                .render(TEMPLATE_TSCONFIG, &template_context)
                .context("failed to render tsconfig template")?,
        },
        GeneratedFile {
            path: PathBuf::from("README.md"),
            contents: tera
                .render(TEMPLATE_README, &template_context)
                .context("failed to render README template")?,
        },
        GeneratedFile {
            path: PathBuf::from("src").join("models.ts"),
            contents: tera
                .render(TEMPLATE_MODELS, &template_context)
                .context("failed to render models template")?,
        },
        GeneratedFile {
            path: PathBuf::from("src").join("client.ts"),
            contents: tera
                .render(TEMPLATE_CLIENT, &template_context)
                .context("failed to render client template")?,
        },
        GeneratedFile {
            path: PathBuf::from("src").join("index.ts"),
            contents: tera
                .render(TEMPLATE_INDEX, &template_context)
                .context("failed to render index template")?,
        },
    ])
}

#[derive(Debug, Serialize)]
struct PackageTemplateContext {
    package_name: String,
    version: String,
    client_imports: String,
    model_blocks: Vec<String>,
    tag_group_blocks: Vec<String>,
    method_blocks: Vec<String>,
    index_model_exports: String,
}

impl PackageTemplateContext {
    fn from_ir(ir: &CoreIr, config: &TypeScriptPackageConfig, tera: &Tera) -> Result<Self> {
        let model_names = sorted_model_names(ir);
        let mut client_imports = model_names
            .iter()
            .map(|name| sanitize_type_name(name))
            .collect::<Vec<_>>();
        client_imports.push("JsonValue".into());
        client_imports.sort();
        client_imports.dedup();

        let model_blocks = sorted_models(ir)
            .into_iter()
            .map(|model| render_model_block(tera, ModelView::from_model(model)))
            .collect::<Result<Vec<_>>>()?;

        let tag_group_blocks = if config.group_by_tag {
            ClientLayout::from_ir(ir)
                .tagged_groups
                .into_iter()
                .map(|(tag_name, operations)| {
                    render_tag_group_block(tera, TagGroupView::new(tag_name, operations))
                })
                .collect::<Result<Vec<_>>>()?
        } else {
            Vec::new()
        };

        let method_blocks = sorted_operations(ir)
            .into_iter()
            .map(|operation| {
                render_client_method_block(tera, OperationMethodView::from_operation(operation))
            })
            .collect::<Result<Vec<_>>>()?;

        let index_model_exports = model_names
            .into_iter()
            .map(|name| sanitize_type_name(&name))
            .collect::<Vec<_>>()
            .join(", ");

        Ok(Self {
            package_name: config.package_name.clone(),
            version: config.version.clone(),
            client_imports: client_imports.join(", "),
            model_blocks,
            tag_group_blocks,
            method_blocks,
            index_model_exports,
        })
    }
}

#[derive(Debug, Serialize)]
struct ModelView {
    type_name: String,
    has_fields: bool,
    is_enum: bool,
    is_alias: bool,
    enum_expression: String,
    alias_expression: String,
    fields_block: String,
}

impl ModelView {
    fn from_model(model: &arvalez_ir::Model) -> Self {
        let enum_expression = model
            .attributes
            .get("enum_values")
            .and_then(|value| value.as_array())
            .map(|values| {
                values
                    .iter()
                    .map(render_typescript_enum_variant)
                    .collect::<Vec<_>>()
                    .join(" | ")
            })
            .unwrap_or_default();
        let is_enum = !enum_expression.is_empty();
        let alias_type_ref = model
            .attributes
            .get("alias_type_ref")
            .cloned()
            .and_then(|value| from_value::<TypeRef>(value).ok());
        let alias_nullable = model
            .attributes
            .get("alias_nullable")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        let is_alias = alias_type_ref.is_some();
        let field_lines = model
            .fields
            .iter()
            .map(|field| {
                let field_name = render_property_name(&field.name);
                let optional_marker = if field.optional { "?" } else { "" };
                format!(
                    "{}{}: {};",
                    field_name,
                    optional_marker,
                    typescript_field_type(&field.type_ref, field.nullable)
                )
            })
            .collect::<Vec<_>>();

        Self {
            type_name: sanitize_type_name(&model.name),
            has_fields: !field_lines.is_empty(),
            is_enum,
            is_alias,
            enum_expression,
            alias_expression: alias_type_ref
                .map(|type_ref| typescript_field_type(&type_ref, alias_nullable))
                .unwrap_or_default(),
            fields_block: indent_block(&field_lines, 2),
        }
    }
}

#[derive(Debug, Serialize)]
struct TagGroupView {
    property_name: String,
    bindings_block: String,
}

impl TagGroupView {
    fn new(tag_name: String, operations: Vec<&Operation>) -> Self {
        let bindings_block = indent_block(
            &operations
                .into_iter()
                .flat_map(|operation| {
                    let method_name = sanitize_identifier(&operation.name);
                    vec![
                        format!("{method_name}: this.{method_name}.bind(this),"),
                        format!(
                            "{}: this.{}.bind(this),",
                            raw_method_name(operation),
                            raw_method_name(operation)
                        ),
                    ]
                })
                .collect::<Vec<_>>(),
            4,
        );

        Self {
            property_name: sanitize_tag_property_name(&tag_name),
            bindings_block,
        }
    }
}

#[derive(Debug, Serialize)]
struct OperationMethodView {
    method_name: String,
    raw_method_name: String,
    args_signature: String,
    return_annotation: String,
    raw_return_annotation: String,
    raw_doc_block: String,
    doc_block: String,
    path_template: String,
    validation_block: String,
    query_block: String,
    headers_block: String,
    request_init_block: String,
    body_block: String,
    raw_response_block: String,
    wrapper_request_call_block: String,
    wrapper_post_request_block: String,
}

impl OperationMethodView {
    fn from_operation(operation: &Operation) -> Self {
        let mut validation_lines = Vec::new();
        for param in operation
            .params
            .iter()
            .filter(|param| matches!(param.location, ParameterLocation::Path))
        {
            if let Some(content_encoding) = content_encoding_attribute(&param.attributes) {
                validation_lines.push(format!(
                    "this.validateStringEncoding({}, {:?}, {:?}, requestOptions);",
                    sanitize_identifier(&param.name),
                    content_encoding,
                    format!("path parameter `{}`", param.name)
                ));
            }
        }

        let mut query_lines = Vec::new();
        let query_params = operation
            .params
            .iter()
            .filter(|param| matches!(param.location, ParameterLocation::Query))
            .collect::<Vec<_>>();
        if !query_params.is_empty() {
            query_lines.push("const baseQuery = new URLSearchParams();".into());
            for param in &query_params {
                let name = sanitize_identifier(&param.name);
                if param.required {
                    if let Some(content_encoding) = content_encoding_attribute(&param.attributes) {
                        validation_lines.push(format!(
                            "this.validateStringEncoding({}, {:?}, {:?}, requestOptions);",
                            name,
                            content_encoding,
                            format!("query parameter `{}`", param.name)
                        ));
                    }
                    query_lines.push(format!(
                        "baseQuery.set({:?}, String({}));",
                        param.name, name
                    ));
                } else {
                    query_lines.push(format!("if ({} !== undefined) {{", name));
                    if let Some(content_encoding) = content_encoding_attribute(&param.attributes) {
                        query_lines.push(format!(
                            "  this.validateStringEncoding({}, {:?}, {:?}, requestOptions);",
                            name,
                            content_encoding,
                            format!("query parameter `{}`", param.name)
                        ));
                    }
                    query_lines.push(format!(
                        "  baseQuery.set({:?}, String({}));",
                        param.name, name
                    ));
                    query_lines.push("}".into());
                }
            }
            query_lines.push("const query = this.mergeQuery(baseQuery, requestOptions);".into());
        } else {
            query_lines.push("const query = this.mergeQuery(undefined, requestOptions);".into());
        }

        let mut headers_lines = Vec::new();
        let header_params = operation
            .params
            .iter()
            .filter(|param| matches!(param.location, ParameterLocation::Header))
            .collect::<Vec<_>>();
        let has_json_body = operation
            .request_body
            .as_ref()
            .is_some_and(|body| body.media_type == "application/json");
        let has_form_body = operation
            .request_body
            .as_ref()
            .is_some_and(|body| body.media_type.starts_with("multipart/form-data"));

        if !header_params.is_empty() || has_json_body {
            headers_lines.push("const headers = this.createHeaders();".into());
            for param in &header_params {
                let name = sanitize_identifier(&param.name);
                if param.required {
                    if let Some(content_encoding) = content_encoding_attribute(&param.attributes) {
                        validation_lines.push(format!(
                            "this.validateStringEncoding({}, {:?}, {:?}, requestOptions);",
                            name,
                            content_encoding,
                            format!("header `{}`", param.name)
                        ));
                    }
                    headers_lines.push(format!("headers.set({:?}, String({}));", param.name, name));
                } else {
                    headers_lines.push(format!("if ({} !== undefined) {{", name));
                    if let Some(content_encoding) = content_encoding_attribute(&param.attributes) {
                        headers_lines.push(format!(
                            "  this.validateStringEncoding({}, {:?}, {:?}, requestOptions);",
                            name,
                            content_encoding,
                            format!("header `{}`", param.name)
                        ));
                    }
                    headers_lines.push(format!(
                        "  headers.set({:?}, String({}));",
                        param.name, name
                    ));
                    headers_lines.push("}".into());
                }
            }
            if has_json_body {
                headers_lines.push("headers.set(\"Content-Type\", \"application/json\");".into());
            }
            headers_lines
                .push("const mergedHeaders = this.mergeHeaders(headers, requestOptions);".into());
        } else {
            headers_lines
                .push("const mergedHeaders = this.mergeHeaders(undefined, requestOptions);".into());
        }

        let request_init_name = "requestInit";
        let request_init_lines = vec![
            format!(
                "const {} = this.createRequestInit(requestOptions);",
                request_init_name
            ),
            format!(
                "{}.method = {:?};",
                request_init_name,
                http_method_string(operation.method)
            ),
            format!(
                "if (mergedHeaders) {{ {}.headers = mergedHeaders; }}",
                request_init_name
            ),
        ];

        let mut body_lines = Vec::new();
        if let Some(request_body) = &operation.request_body {
            let body_name = "body";
            if request_body.required {
                if let Some(content_encoding) = content_encoding_attribute(&request_body.attributes) {
                    validation_lines.push(format!(
                        "this.validateStringEncoding({body_name}, {:?}, \"request body\", requestOptions);",
                        content_encoding
                    ));
                }
                if has_json_body {
                    body_lines.push(format!(
                        "{request_init_name}.body = JSON.stringify({body_name});"
                    ));
                } else if has_form_body {
                    body_lines.push(format!(
                        "{request_init_name}.body = this.toFormData({body_name} as unknown as Record<string, unknown>);"
                    ));
                } else {
                    body_lines.push(format!(
                        "{request_init_name}.body = {body_name} as BodyInit;"
                    ));
                }
            } else {
                body_lines.push(format!("if ({body_name} !== undefined) {{"));
                if let Some(content_encoding) = content_encoding_attribute(&request_body.attributes) {
                    body_lines.push(format!(
                        "  this.validateStringEncoding({body_name}, {:?}, \"request body\", requestOptions);",
                        content_encoding
                    ));
                }
                if has_json_body {
                    body_lines.push(format!(
                        "  {request_init_name}.body = JSON.stringify({body_name});"
                    ));
                } else if has_form_body {
                    body_lines.push(format!(
                        "  {request_init_name}.body = this.toFormData({body_name} as unknown as Record<string, unknown>);"
                    ));
                } else {
                    body_lines.push(format!(
                        "  {request_init_name}.body = {body_name} as BodyInit;"
                    ));
                }
                body_lines.push("}".into());
            }
        }

        body_lines.push(format!(
            "const response = await this.fetchFn(this.buildUrl(path, query), {});",
            request_init_name
        ));
        let raw_response_block = indent_block(&body_lines, 4);
        let wrapper_request_call_block = indent_block(
            &[format!(
                "const response = await this.{}({});",
                raw_method_name(operation),
                build_wrapper_forward_arguments(operation)
            )],
            4,
        );
        let wrapper_post_request_block = indent_block(
            &[
                "await this.handleError(response, requestOptions);".into(),
                format!(
                    "return await this.parseResponse<{}>(response, {}, requestOptions);",
                    operation_return_type(operation),
                    operation
                        .responses
                        .iter()
                        .find(|response| response.status.starts_with('2'))
                        .and_then(|response| content_encoding_attribute(&response.attributes))
                        .map(|encoding| format!("{encoding:?}"))
                        .unwrap_or_else(|| "undefined".into())
                ),
            ],
            4,
        );

        Self {
            method_name: sanitize_identifier(&operation.name),
            raw_method_name: raw_method_name(operation),
            args_signature: build_method_args(operation).join(", "),
            return_annotation: operation_return_type(operation),
            raw_return_annotation: "Response".into(),
            raw_doc_block: render_typescript_doc_block(operation, true),
            doc_block: render_typescript_doc_block(operation, false),
            path_template: render_typescript_path(&operation.path),
            validation_block: indent_block(&validation_lines, 4),
            query_block: indent_block(&query_lines, 4),
            headers_block: indent_block(&headers_lines, 4),
            request_init_block: indent_block(&request_init_lines, 4),
            body_block: raw_response_block,
            raw_response_block: "    return response;".into(),
            wrapper_request_call_block,
            wrapper_post_request_block,
        }
    }
}

fn content_encoding_attribute(attributes: &BTreeMap<String, Value>) -> Option<&str> {
    attributes.get("content_encoding").and_then(Value::as_str)
}

fn render_typescript_doc_block(operation: &Operation, raw: bool) -> String {
    let mut lines = Vec::new();
    if let Some(summary) = operation.attributes.get("summary").and_then(Value::as_str) {
        let summary = summary.trim();
        if !summary.is_empty() {
            lines.push(summary.to_owned());
        }
    }
    if raw {
        lines.push("Returns the raw HTTP response without parsing it or throwing for HTTP errors.".into());
    }
    for param in &operation.params {
        if let Some(description) = param.attributes.get("description").and_then(Value::as_str) {
            let description = description.trim();
            if !description.is_empty() {
                lines.push(format!(
                    "@param {} {}",
                    sanitize_identifier(&param.name),
                    sanitize_doc_text(description)
                ));
            }
        }
    }

    if lines.is_empty() {
        String::new()
    } else {
        let mut block = vec!["/**".to_owned()];
        for line in lines {
            block.push(format!(" * {line}"));
        }
        block.push(" */".into());
        indent_block(&block, 2)
    }
}

fn sanitize_doc_text(value: &str) -> String {
    value.replace("*/", "*\\/")
}

fn render_model_block(tera: &Tera, model: ModelView) -> Result<String> {
    let mut context = TeraContext::new();
    context.insert("model", &model);
    tera.render(TEMPLATE_MODEL_INTERFACE, &context)
        .context("failed to render model interface partial")
}

fn render_tag_group_block(tera: &Tera, tag_group: TagGroupView) -> Result<String> {
    let mut context = TeraContext::new();
    context.insert("tag_group", &tag_group);
    tera.render(TEMPLATE_TAG_GROUP, &context)
        .context("failed to render tag group partial")
}

fn render_client_method_block(tera: &Tera, operation: OperationMethodView) -> Result<String> {
    let mut context = TeraContext::new();
    context.insert("operation", &operation);
    tera.render(TEMPLATE_CLIENT_METHOD, &context)
        .context("failed to render client method partial")
}

fn sorted_model_names(ir: &CoreIr) -> Vec<String> {
    sorted_models(ir)
        .into_iter()
        .map(|model| model.name.clone())
        .collect()
}

fn build_method_args(operation: &Operation) -> Vec<String> {
    let mut args = Vec::new();
    for param in operation.params.iter().filter(|param| param.required) {
        args.push(format!(
            "{}: {}",
            sanitize_identifier(&param.name),
            typescript_type_ref(&param.type_ref)
        ));
    }
    if let Some(request_body) = &operation.request_body
        && request_body.required
    {
        args.push(format!(
            "body: {}",
            request_body
                .type_ref
                .as_ref()
                .map(typescript_type_ref)
                .unwrap_or_else(|| "unknown".into())
        ));
    }
    for param in operation.params.iter().filter(|param| !param.required) {
        args.push(format!(
            "{}?: {}",
            sanitize_identifier(&param.name),
            typescript_type_ref(&param.type_ref)
        ));
    }
    if let Some(request_body) = &operation.request_body
        && !request_body.required
    {
        args.push(format!(
            "body?: {}",
            request_body
                .type_ref
                .as_ref()
                .map(typescript_type_ref)
                .unwrap_or_else(|| "unknown".into())
        ));
    }
    args.push("requestOptions?: RequestOptions".into());
    args
}

fn operation_return_type(operation: &Operation) -> String {
    operation
        .responses
        .iter()
        .find(|response| response.status.starts_with('2'))
        .and_then(|response| response.type_ref.as_ref())
        .map(typescript_type_ref)
        .unwrap_or_else(|| "void".into())
}

fn raw_method_name(operation: &Operation) -> String {
    format!("_{}Raw", sanitize_identifier(&operation.name))
}

fn build_wrapper_forward_arguments(operation: &Operation) -> String {
    let mut args = Vec::new();
    for param in operation.params.iter().filter(|param| param.required) {
        args.push(sanitize_identifier(&param.name));
    }
    if let Some(request_body) = &operation.request_body
        && request_body.required
    {
        let _ = request_body;
        args.push("body".into());
    }
    for param in operation.params.iter().filter(|param| !param.required) {
        args.push(sanitize_identifier(&param.name));
    }
    if let Some(request_body) = &operation.request_body
        && !request_body.required
    {
        let _ = request_body;
        args.push("body".into());
    }
    args.push("requestOptions".into());
    args.join(", ")
}

fn typescript_field_type(type_ref: &TypeRef, nullable: bool) -> String {
    let mut ty = typescript_type_ref(type_ref);
    if nullable {
        ty.push_str(" | null");
    }
    ty
}

fn typescript_type_ref(type_ref: &TypeRef) -> String {
    match type_ref {
        TypeRef::Primitive { name } => match name.as_str() {
            "string" => "string".into(),
            "integer" | "number" => "number".into(),
            "boolean" => "boolean".into(),
            "binary" => "Blob".into(),
            "null" => "null".into(),
            "any" | "object" => "JsonValue".into(),
            _ => "unknown".into(),
        },
        TypeRef::Named { name } => sanitize_type_name(name),
        TypeRef::Array { item } => format!("{}[]", typescript_type_ref(item)),
        TypeRef::Map { value } => format!("Record<string, {}>", typescript_type_ref(value)),
        TypeRef::Union { variants } => variants
            .iter()
            .map(typescript_type_ref)
            .collect::<Vec<_>>()
            .join(" | "),
    }
}

fn render_typescript_enum_variant(value: &Value) -> String {
    match value {
        Value::String(value) => format!("{value:?}"),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::Null => "null".into(),
        _ => "unknown".into(),
    }
}

fn http_method_string(method: HttpMethod) -> &'static str {
    match method {
        HttpMethod::Get => "GET",
        HttpMethod::Post => "POST",
        HttpMethod::Put => "PUT",
        HttpMethod::Patch => "PATCH",
        HttpMethod::Delete => "DELETE",
    }
}

fn render_typescript_path(path: &str) -> String {
    let mut result = String::from("`");
    let mut chars = path.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '{' => {
                let mut name = String::new();
                while let Some(next) = chars.peek() {
                    if *next == '}' {
                        chars.next();
                        break;
                    }
                    name.push(*next);
                    chars.next();
                }
                result.push_str("${");
                result.push_str(&sanitize_identifier(&name));
                result.push('}');
            }
            '`' => result.push_str("\\`"),
            _ => result.push(ch),
        }
    }
    result.push('`');
    result
}

fn sanitize_type_name(name: &str) -> String {
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

fn sanitize_identifier(name: &str) -> String {
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

fn sanitize_tag_property_name(name: &str) -> String {
    let property = split_words(name).join("_");
    if property.is_empty() {
        "default".into()
    } else if is_typescript_keyword(&property) {
        format!("{property}_")
    } else {
        property
    }
}

fn render_property_name(name: &str) -> String {
    if is_valid_typescript_identifier(name) && !is_typescript_keyword(name) {
        name.into()
    } else {
        format!("{name:?}")
    }
}

fn is_valid_typescript_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    match chars.next() {
        Some(ch) if ch == '_' || ch == '$' || ch.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|ch| ch == '_' || ch == '$' || ch.is_ascii_alphanumeric())
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

fn is_typescript_keyword(value: &str) -> bool {
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

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use arvalez_ir::{Attributes, Field, Parameter, RequestBody, Response};
    use serde_json::json;
    use tempfile::tempdir;

    fn sample_ir() -> CoreIr {
        CoreIr {
            models: vec![arvalez_ir::Model {
                id: "model.widget".into(),
                name: "Widget".into(),
                fields: vec![
                    Field::new("id", TypeRef::primitive("string")),
                    Field {
                        name: "count".into(),
                        type_ref: TypeRef::primitive("integer"),
                        optional: true,
                        nullable: false,
                        attributes: Attributes::default(),
                    },
                ],
                attributes: Attributes::default(),
                source: None,
            }],
            operations: vec![Operation {
                id: "operation.get_widget".into(),
                name: "get_widget".into(),
                method: HttpMethod::Get,
                path: "/widgets/{widget_id}".into(),
                params: vec![
                    Parameter {
                        name: "widget_id".into(),
                        location: ParameterLocation::Path,
                        type_ref: TypeRef::primitive("string"),
                        required: true,
                        attributes: Attributes::from([(
                            "description".into(),
                            Value::String("Unique widget identifier.".into()),
                        )]),
                    },
                    Parameter {
                        name: "include_count".into(),
                        location: ParameterLocation::Query,
                        type_ref: TypeRef::primitive("boolean"),
                        required: false,
                        attributes: Attributes::default(),
                    },
                ],
                request_body: Some(RequestBody {
                    required: false,
                    media_type: "application/json".into(),
                    type_ref: Some(TypeRef::named("Widget")),
                    attributes: Attributes::default(),
                }),
                responses: vec![Response {
                    status: "200".into(),
                    media_type: Some("application/json".into()),
                    type_ref: Some(TypeRef::named("Widget")),
                    attributes: Attributes::default(),
                }],
                attributes: Attributes::from([("tags".into(), json!(["widgets"]))]),
                source: None,
            }],
            ..Default::default()
        }
    }

    #[test]
    fn renders_basic_typescript_package() {
        let files = generate_typescript_package(&sample_ir(), &TypeScriptPackageConfig::new("@demo/client"))
            .expect("package should render");

        let package_json = files
            .iter()
            .find(|file| file.path.ends_with("package.json"))
            .expect("package.json");
        let models = files
            .iter()
            .find(|file| file.path.ends_with("models.ts"))
            .expect("models.ts");
        let client = files
            .iter()
            .find(|file| file.path.ends_with("client.ts"))
            .expect("client.ts");
        let index = files
            .iter()
            .find(|file| file.path.ends_with("index.ts"))
            .expect("index.ts");

        assert!(package_json.contents.contains("\"name\": \"@demo/client\""));
        assert!(models.contents.contains("export interface Widget"));
        assert!(models.contents.contains("count?: number;"));
        assert!(client.contents.contains("export class ApiClient"));
        assert!(client.contents.contains("export interface RequestOptions"));
        assert!(
            client.contents.contains(
                "export type ErrorHandler = (response: globalThis.Response) => void | Promise<void>;"
            )
        );
        assert!(client.contents.contains("async _getWidgetRaw("));
        assert!(client.contents.contains("async getWidget("));
        assert!(
            client
                .contents
                .contains("@param widgetId Unique widget identifier.")
        );
        assert!(client.contents.contains("requestOptions?: RequestOptions"));
        assert!(client.contents.contains("onError?: ErrorHandler;"));
        assert!(
            client
                .contents
                .contains("const baseQuery = new URLSearchParams();")
        );
        assert!(
            client
                .contents
                .contains("const query = this.mergeQuery(baseQuery, requestOptions);")
        );
        assert!(client.contents.contains("body?: Widget"));
        assert!(
            client
                .contents
                .contains("const response = await this._getWidgetRaw(")
        );
        assert!(
            client
                .contents
                .contains("await this.handleError(response, requestOptions);")
        );
        assert!(
            index
                .contents
                .contains("export type { ApiClientOptions, ErrorHandler, RequestOptions }")
        );
    }

    #[test]
    fn renders_aliases_and_enums_as_typescript_types() {
        let ir = CoreIr {
            models: vec![
                arvalez_ir::Model {
                    id: "model.widget_path".into(),
                    name: "WidgetPath".into(),
                    fields: vec![],
                    attributes: Attributes::from([(
                        "alias_type_ref".into(),
                        json!(TypeRef::primitive("string")),
                    )]),
                    source: None,
                },
                arvalez_ir::Model {
                    id: "model.widget_status".into(),
                    name: "WidgetStatus".into(),
                    fields: vec![],
                    attributes: Attributes::from([(
                        "enum_values".into(),
                        json!(["READY", "PAUSED"]),
                    )]),
                    source: None,
                },
            ],
            ..Default::default()
        };

        let files = generate_typescript_package(&ir, &TypeScriptPackageConfig::new("@demo/client"))
            .expect("package should render");
        let models = files
            .iter()
            .find(|file| file.path.ends_with("models.ts"))
            .expect("models.ts");

        assert!(models.contents.contains("export type WidgetPath = string;"));
        assert!(models.contents.contains("export type WidgetStatus = \"READY\" | \"PAUSED\";"));
    }

    #[test]
    fn groups_operations_by_tag_when_enabled() {
        let files = generate_typescript_package(
            &sample_ir(),
            &TypeScriptPackageConfig::new("@demo/client").with_group_by_tag(true),
        )
        .expect("package should render");
        let client = files
            .iter()
            .find(|file| file.path.ends_with("client.ts"))
            .expect("client.ts");

        assert!(client.contents.contains("readonly widgets = {"));
        assert!(
            client
                .contents
                .contains("getWidget: this.getWidget.bind(this),")
        );
        assert!(
            client
                .contents
                .contains("_getWidgetRaw: this._getWidgetRaw.bind(this),")
        );
    }

    #[test]
    fn supports_selective_template_overrides() {
        let tempdir = tempdir().expect("tempdir");
        let partial_dir = tempdir.path().join("partials");
        fs::create_dir_all(&partial_dir).expect("partials dir");
        fs::write(
            partial_dir.join("tag_group.ts.tera"),
            "readonly {{ tag_group.property_name }} = { overridden: true };\n",
        )
        .expect("override template");

        let files = generate_typescript_package(
            &sample_ir(),
            &TypeScriptPackageConfig::new("@demo/client")
                .with_group_by_tag(true)
                .with_template_dir(Some(tempdir.path().to_path_buf())),
        )
        .expect("package should render");
        let client = files
            .iter()
            .find(|file| file.path.ends_with("client.ts"))
            .expect("client.ts");

        assert!(
            client
                .contents
                .contains("readonly widgets = { overridden: true };")
        );
    }
}
