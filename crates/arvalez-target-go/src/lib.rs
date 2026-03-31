use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use arvalez_ir::{CoreIr, HttpMethod, Operation, ParameterLocation, RequestBody, TypeRef};
pub use arvalez_target_core::GeneratedFile;
pub use arvalez_target_core::write_files as write_go_package;
use arvalez_target_core::{
    ClientLayout as SharedClientLayout, indent_block, load_templates,
    sorted_models,
};
use serde::Serialize;
use serde_json::Value;
use tera::{Context as TeraContext, Tera};

const TEMPLATE_GO_MOD: &str = "package/go.mod.tera";
const TEMPLATE_README: &str = "package/README.md.tera";
const TEMPLATE_MODELS: &str = "package/models.go.tera";
const TEMPLATE_CLIENT: &str = "package/client.go.tera";
const TEMPLATE_MODEL_STRUCT: &str = "partials/model_struct.go.tera";
const TEMPLATE_SERVICE: &str = "partials/service.go.tera";
const TEMPLATE_CLIENT_METHOD: &str = "partials/client_method.go.tera";

const BUILTIN_TEMPLATES: &[(&str, &str)] = &[
    (
        TEMPLATE_GO_MOD,
        include_str!("../templates/package/go.mod.tera"),
    ),
    (
        TEMPLATE_README,
        include_str!("../templates/package/README.md.tera"),
    ),
    (
        TEMPLATE_MODELS,
        include_str!("../templates/package/models.go.tera"),
    ),
    (
        TEMPLATE_CLIENT,
        include_str!("../templates/package/client.go.tera"),
    ),
    (
        TEMPLATE_MODEL_STRUCT,
        include_str!("../templates/partials/model_struct.go.tera"),
    ),
    (
        TEMPLATE_SERVICE,
        include_str!("../templates/partials/service.go.tera"),
    ),
    (
        TEMPLATE_CLIENT_METHOD,
        include_str!("../templates/partials/client_method.go.tera"),
    ),
];

const OVERRIDABLE_TEMPLATES: &[&str] = &[
    TEMPLATE_GO_MOD,
    TEMPLATE_README,
    TEMPLATE_MODELS,
    TEMPLATE_CLIENT,
    TEMPLATE_MODEL_STRUCT,
    TEMPLATE_SERVICE,
    TEMPLATE_CLIENT_METHOD,
];

#[derive(Debug, Clone)]
pub struct GoPackageConfig {
    pub module_path: String,
    pub package_name: String,
    pub version: String,
    pub template_dir: Option<PathBuf>,
    pub group_by_tag: bool,
}

impl GoPackageConfig {
    pub fn new(module_path: impl Into<String>) -> Self {
        let module_path = module_path.into();
        let package_name = default_package_name(&module_path);
        Self {
            module_path,
            package_name,
            version: "0.1.0".into(),
            template_dir: None,
            group_by_tag: false,
        }
    }

    pub fn with_package_name(mut self, package_name: impl Into<String>) -> Self {
        self.package_name = sanitize_package_name(&package_name.into());
        self
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

pub fn generate_go_package(ir: &CoreIr, config: &GoPackageConfig) -> Result<Vec<GeneratedFile>> {
    let tera = load_templates(config.template_dir.as_deref(), BUILTIN_TEMPLATES, OVERRIDABLE_TEMPLATES)?;
    let package_context = PackageTemplateContext::from_ir(ir, config, &tera)?;
    let mut context = TeraContext::new();
    context.insert("package", &package_context);

    Ok(vec![
        GeneratedFile {
            path: PathBuf::from("go.mod"),
            contents: tera
                .render(TEMPLATE_GO_MOD, &context)
                .context("failed to render go.mod template")?,
        },
        GeneratedFile {
            path: PathBuf::from("README.md"),
            contents: tera
                .render(TEMPLATE_README, &context)
                .context("failed to render README template")?,
        },
        GeneratedFile {
            path: PathBuf::from("models.go"),
            contents: tera
                .render(TEMPLATE_MODELS, &context)
                .context("failed to render models template")?,
        },
        GeneratedFile {
            path: PathBuf::from("client.go"),
            contents: tera
                .render(TEMPLATE_CLIENT, &context)
                .context("failed to render client template")?,
        },
    ])
}

#[derive(Debug, Serialize)]
struct PackageTemplateContext {
    module_path: String,
    package_name: String,
    version: String,
    model_blocks: Vec<String>,
    service_fields_block: String,
    service_init_block: String,
    client_method_blocks: String,
    service_blocks: Vec<String>,
}

impl PackageTemplateContext {
    fn from_ir(ir: &CoreIr, config: &GoPackageConfig, tera: &Tera) -> Result<Self> {
        let model_blocks = sorted_models(ir)
            .into_iter()
            .map(|model| render_model_block(tera, ModelView::from_model(model)))
            .collect::<Result<Vec<_>>>()?;

        let layout = ClientLayout::from_ir(ir);

        let client_method_blocks = render_method_blocks(
            tera,
            if config.group_by_tag {
                layout
                    .untagged_operations
                    .iter()
                    .map(|operation| OperationMethodView::client_method(operation))
                    .collect::<Vec<_>>()
            } else {
                layout
                    .all_operations
                    .iter()
                    .map(|operation| OperationMethodView::client_method(operation))
                    .collect::<Vec<_>>()
            },
        )?;

        let service_fields_block = if config.group_by_tag {
            indent_block(
                &layout
                    .tag_groups
                    .iter()
                    .map(|group| format!("{} *{}", group.field_name, group.struct_name))
                    .collect::<Vec<_>>(),
                4,
            )
        } else {
            String::new()
        };

        let service_init_block = if config.group_by_tag {
            indent_block(
                &layout
                    .tag_groups
                    .iter()
                    .map(|group| {
                        format!(
                            "client.{} = &{}{{client: client}}",
                            group.field_name, group.struct_name
                        )
                    })
                    .collect::<Vec<_>>(),
                4,
            )
        } else {
            String::new()
        };

        let service_blocks = if config.group_by_tag {
            layout
                .tag_groups
                .iter()
                .map(|group| render_service_block(tera, ServiceView::from_group(group, tera)))
                .collect::<Result<Vec<_>>>()?
        } else {
            Vec::new()
        };

        Ok(Self {
            module_path: config.module_path.clone(),
            package_name: config.package_name.clone(),
            version: config.version.clone(),
            model_blocks,
            service_fields_block,
            service_init_block,
            client_method_blocks,
            service_blocks,
        })
    }
}

#[derive(Debug)]
struct ClientLayout<'a> {
    all_operations: Vec<&'a Operation>,
    untagged_operations: Vec<&'a Operation>,
    tag_groups: Vec<TagGroup<'a>>,
}

impl<'a> ClientLayout<'a> {
    fn from_ir(ir: &'a CoreIr) -> Self {
        let shared = SharedClientLayout::from_ir(ir);
        let tag_groups = shared
            .tagged_groups
            .into_iter()
            .map(|(tag, ops)| TagGroup::new(tag, ops))
            .collect();
        Self {
            all_operations: shared.all_operations,
            untagged_operations: shared.untagged_operations,
            tag_groups,
        }
    }
}

#[derive(Debug)]
struct TagGroup<'a> {
    field_name: String,
    struct_name: String,
    operations: Vec<&'a Operation>,
}

impl<'a> TagGroup<'a> {
    fn new(tag: String, operations: Vec<&'a Operation>) -> Self {
        Self {
            field_name: sanitize_exported_identifier(&tag),
            struct_name: format!("{}Service", sanitize_exported_identifier(&tag)),
            operations,
        }
    }
}

#[derive(Debug, Serialize)]
struct ModelView {
    struct_name: String,
    has_fields: bool,
    fields_block: String,
}

impl ModelView {
    fn from_model(model: &arvalez_ir::Model) -> Self {
        let field_lines = model
            .fields
            .iter()
            .map(ModelFieldView::from_field)
            .map(|field| field.declaration)
            .collect::<Vec<_>>();

        Self {
            struct_name: sanitize_exported_identifier(&model.name),
            has_fields: !field_lines.is_empty(),
            fields_block: indent_block(&field_lines, 4),
        }
    }
}

#[derive(Debug)]
struct ModelFieldView {
    declaration: String,
}

impl ModelFieldView {
    fn from_field(field: &arvalez_ir::Field) -> Self {
        let field_name = sanitize_exported_identifier(&field.name);
        let field_type = go_field_type(&field.type_ref, field.optional, field.nullable);
        let tag = if field.optional {
            format!("`json:\"{},omitempty\"`", field.name)
        } else {
            format!("`json:\"{}\"`", field.name)
        };

        Self {
            declaration: format!("{field_name} {field_type} {tag}"),
        }
    }
}

#[derive(Debug, Serialize)]
struct ServiceView {
    struct_name: String,
    methods_block: String,
}

impl ServiceView {
    fn from_group(group: &TagGroup<'_>, tera: &Tera) -> Result<Self> {
        Ok(Self {
            struct_name: group.struct_name.clone(),
            methods_block: render_method_blocks(
                tera,
                group
                    .operations
                    .iter()
                    .map(|operation| {
                        OperationMethodView::service_method(operation, &group.struct_name)
                    })
                    .collect::<Vec<_>>(),
            )?,
        })
    }
}

#[derive(Debug, Serialize)]
struct OperationMethodView {
    receiver_name: String,
    receiver_type: String,
    client_expression: String,
    method_name: String,
    raw_method_name: String,
    args_signature: String,
    return_signature: String,
    raw_doc_block: String,
    doc_block: String,
    raw_block: String,
    wrapper_request_call_line: String,
    wrapper_error_block: String,
    wrapper_post_request_block: String,
}

impl OperationMethodView {
    fn client_method(operation: &Operation) -> Self {
        Self::from_operation(operation, "c", "Client", "c")
    }

    fn service_method(operation: &Operation, service_name: &str) -> Self {
        Self::from_operation(operation, "s", service_name, "s.client")
    }

    fn from_operation(
        operation: &Operation,
        receiver_name: &str,
        receiver_type: &str,
        client_expression: &str,
    ) -> Self {
        let return_shape = go_return_shape(operation);
        let wrapper_forward_arguments = build_forward_arguments(operation);

        Self {
            receiver_name: receiver_name.into(),
            receiver_type: receiver_type.into(),
            client_expression: client_expression.into(),
            method_name: sanitize_exported_identifier(&operation.name),
            raw_method_name: format!("{}Raw", sanitize_exported_identifier(&operation.name)),
            args_signature: build_method_args(operation).join(", "),
            return_signature: return_shape.signature.clone(),
            raw_doc_block: render_go_doc_block(operation, true),
            doc_block: render_go_doc_block(operation, false),
            raw_block: build_raw_block(operation),
            wrapper_request_call_line: format!(
                "response, err := {receiver_name}.{}({wrapper_forward_arguments})",
                format!("{}Raw", sanitize_exported_identifier(&operation.name))
            ),
            wrapper_error_block: indent_block(&return_shape.raw_error_lines, 4),
            wrapper_post_request_block: indent_block(&return_shape.post_response_lines, 4),
        }
    }
}

fn render_go_doc_block(operation: &Operation, raw: bool) -> String {
    let method_name = sanitize_exported_identifier(&operation.name);
    let prefix = if raw {
        format!("{method_name}Raw")
    } else {
        method_name
    };

    let mut lines = Vec::new();
    if let Some(summary) = operation.attributes.get("summary").and_then(Value::as_str) {
        let summary = summary.trim();
        if !summary.is_empty() {
            lines.push(format!("{prefix} {}", sanitize_go_comment_text(summary)));
        }
    }
    if raw {
        lines.push(format!(
            "{prefix} returns the raw HTTP response without decoding it or converting HTTP errors."
        ));
    }
    for param in &operation.params {
        if let Some(description) = param.attributes.get("description").and_then(Value::as_str) {
            let description = description.trim();
            if !description.is_empty() {
                lines.push(format!(
                    "{} parameter {}: {}",
                    prefix,
                    sanitize_identifier(&param.name),
                    sanitize_go_comment_text(description)
                ));
            }
        }
    }

    if lines.is_empty() {
        String::new()
    } else {
        lines.into_iter()
            .map(|line| format!("// {line}"))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn sanitize_go_comment_text(value: &str) -> String {
    value.replace('\n', " ").replace('\r', " ")
}

#[derive(Debug, Clone)]
struct GoReturnShape {
    signature: String,
    raw_error_lines: Vec<String>,
    post_response_lines: Vec<String>,
}

fn go_return_shape(operation: &Operation) -> GoReturnShape {
    let success = operation
        .responses
        .iter()
        .find(|response| response.status.starts_with('2'));
    let response_content_encoding = success.and_then(|response| content_encoding_attribute(&response.attributes));

    match success.and_then(|response| response.type_ref.as_ref()) {
        Some(type_ref) => {
            let result_type = go_result_type(type_ref);
            let zero_lines = if returns_nil_on_error(type_ref) {
                vec![
                    "if err != nil {".into(),
                    "    return nil, err".into(),
                    "}".into(),
                ]
            } else {
                vec![
                    "if err != nil {".into(),
                    format!("    var zero {result_type}"),
                    "    return zero, err".into(),
                    "}".into(),
                ]
            };

            let mut post_response_lines = vec![
                "defer response.Body.Close()".into(),
                "if err := client.handleError(response, requestOptions); err != nil {".into(),
            ];
            if returns_nil_on_error(type_ref) {
                post_response_lines.push("    return nil, err".into());
            } else {
                post_response_lines.push(format!("    var zero {result_type}"));
                post_response_lines.push("    return zero, err".into());
            }
            post_response_lines.push("}".into());

            let decode_type = go_decode_type(type_ref);
            post_response_lines.push(format!("var result {decode_type}"));
            post_response_lines.push(
                "if err := client.decodeJSONResponse(response, &result); err != nil {".into(),
            );
            if returns_nil_on_error(type_ref) {
                post_response_lines.push("    return nil, err".into());
            } else {
                post_response_lines.push(format!("    var zero {result_type}"));
                post_response_lines.push("    return zero, err".into());
            }
            post_response_lines.push("}".into());
            if let Some(content_encoding) = response_content_encoding {
                post_response_lines.push(format!(
                    "if err := client.validateStringEncoding(result, {:?}, \"response body\", requestOptions); err != nil {{",
                    content_encoding
                ));
                if returns_nil_on_error(type_ref) {
                    post_response_lines.push("    return nil, err".into());
                } else {
                    post_response_lines.push(format!("    var zero {result_type}"));
                    post_response_lines.push("    return zero, err".into());
                }
                post_response_lines.push("}".into());
            }
            if returns_pointer_result(type_ref) {
                post_response_lines.push("return &result, nil".into());
            } else {
                post_response_lines.push("return result, nil".into());
            }

            GoReturnShape {
                signature: format!("({result_type}, error)"),
                raw_error_lines: zero_lines,
                post_response_lines,
            }
        }
        None => GoReturnShape {
            signature: "error".into(),
            raw_error_lines: vec![
                "if err != nil {".into(),
                "    return err".into(),
                "}".into(),
            ],
            post_response_lines: vec![
                "defer response.Body.Close()".into(),
                "if err := client.handleError(response, requestOptions); err != nil {".into(),
                "    return err".into(),
                "}".into(),
                "return nil".into(),
            ],
        },
    }
}

fn build_raw_block(operation: &Operation) -> String {
    let mut lines = vec![
        render_go_path_line(&operation.path, &operation.params),
        "query := url.Values{}".into(),
    ];

    for param in operation
        .params
        .iter()
        .filter(|param| matches!(param.location, ParameterLocation::Path))
    {
        let name = sanitize_identifier(&param.name);
        if let Some(content_encoding) = content_encoding_attribute(&param.attributes) {
            lines.push(format!(
                "if err := client.validateStringEncoding({name}, {:?}, {:?}, requestOptions); err != nil {{",
                content_encoding,
                format!("path parameter `{}`", param.name)
            ));
            lines.push("    return nil, err".into());
            lines.push("}".into());
        }
    }

    for param in operation
        .params
        .iter()
        .filter(|param| matches!(param.location, ParameterLocation::Query))
    {
        let name = sanitize_identifier(&param.name);
        if param.required {
            if let Some(content_encoding) = content_encoding_attribute(&param.attributes) {
                lines.push(format!(
                    "if err := client.validateStringEncoding({name}, {:?}, {:?}, requestOptions); err != nil {{",
                    content_encoding,
                    format!("query parameter `{}`", param.name)
                ));
                lines.push("    return nil, err".into());
                lines.push("}".into());
            }
            lines.push(format!("query.Set({:?}, fmt.Sprint({name}))", param.name));
        } else {
            lines.push(format!("if {name} != nil {{"));
            if let Some(content_encoding) = content_encoding_attribute(&param.attributes) {
                lines.push(format!(
                    "    if err := client.validateStringEncoding({name}, {:?}, {:?}, requestOptions); err != nil {{",
                    content_encoding,
                    format!("query parameter `{}`", param.name)
                ));
                lines.push("        return nil, err".into());
                lines.push("    }".into());
            }
            lines.push(format!(
                "    query.Set({:?}, fmt.Sprint(*{name}))",
                param.name
            ));
            lines.push("}".into());
        }
    }
    lines.push("query = client.mergeQuery(query, requestOptions)".into());

    lines.push("headers := http.Header{}".into());
    for param in operation
        .params
        .iter()
        .filter(|param| matches!(param.location, ParameterLocation::Header))
    {
        let name = sanitize_identifier(&param.name);
        if param.required {
            if let Some(content_encoding) = content_encoding_attribute(&param.attributes) {
                lines.push(format!(
                    "if err := client.validateStringEncoding({name}, {:?}, {:?}, requestOptions); err != nil {{",
                    content_encoding,
                    format!("header `{}`", param.name)
                ));
                lines.push("    return nil, err".into());
                lines.push("}".into());
            }
            lines.push(format!("headers.Set({:?}, fmt.Sprint({name}))", param.name));
        } else {
            lines.push(format!("if {name} != nil {{"));
            if let Some(content_encoding) = content_encoding_attribute(&param.attributes) {
                lines.push(format!(
                    "    if err := client.validateStringEncoding({name}, {:?}, {:?}, requestOptions); err != nil {{",
                    content_encoding,
                    format!("header `{}`", param.name)
                ));
                lines.push("        return nil, err".into());
                lines.push("    }".into());
            }
            lines.push(format!(
                "    headers.Set({:?}, fmt.Sprint(*{name}))",
                param.name
            ));
            lines.push("}".into());
        }
    }

    lines.push("cookies := []*http.Cookie{}".into());
    for param in operation
        .params
        .iter()
        .filter(|param| matches!(param.location, ParameterLocation::Cookie))
    {
        let name = sanitize_identifier(&param.name);
        if param.required {
            if let Some(content_encoding) = content_encoding_attribute(&param.attributes) {
                lines.push(format!(
                    "if err := client.validateStringEncoding({name}, {:?}, {:?}, requestOptions); err != nil {{",
                    content_encoding,
                    format!("cookie `{}`", param.name)
                ));
                lines.push("    return nil, err".into());
                lines.push("}".into());
            }
            lines.push(format!(
                "cookies = append(cookies, &http.Cookie{{Name: {:?}, Value: fmt.Sprint({name})}})",
                param.name
            ));
        } else {
            lines.push(format!("if {name} != nil {{"));
            if let Some(content_encoding) = content_encoding_attribute(&param.attributes) {
                lines.push(format!(
                    "    if err := client.validateStringEncoding({name}, {:?}, {:?}, requestOptions); err != nil {{",
                    content_encoding,
                    format!("cookie `{}`", param.name)
                ));
                lines.push("        return nil, err".into());
                lines.push("    }".into());
            }
            lines.push(format!(
                "    cookies = append(cookies, &http.Cookie{{Name: {:?}, Value: fmt.Sprint(*{name})}})",
                param.name
            ));
            lines.push("}".into());
        }
    }
    lines.push("cookies = client.mergeCookies(cookies, requestOptions)".into());

    lines.push("var bodyReader io.Reader".into());
    lines.push("var err error".into());
    if let Some(request_body) = &operation.request_body {
        match classify_request_body(request_body) {
            RequestBodyKind::Json => {
                lines.push("headers.Set(\"Content-Type\", \"application/json\")".into());
                if request_body.required {
                    if let Some(content_encoding) = content_encoding_attribute(&request_body.attributes) {
                        lines.push(format!(
                            "if err := client.validateStringEncoding(body, {:?}, \"request body\", requestOptions); err != nil {{",
                            content_encoding
                        ));
                        lines.push("    return nil, err".into());
                        lines.push("}".into());
                    }
                    lines.push("bodyReader, err = client.encodeJSONBody(body)".into());
                    lines.push("if err != nil {".into());
                    lines.push("    return nil, err".into());
                    lines.push("}".into());
                } else {
                    lines.push("if body != nil {".into());
                    if let Some(content_encoding) = content_encoding_attribute(&request_body.attributes) {
                        lines.push(format!(
                            "    if err := client.validateStringEncoding(body, {:?}, \"request body\", requestOptions); err != nil {{",
                            content_encoding
                        ));
                        lines.push("        return nil, err".into());
                        lines.push("    }".into());
                    }
                    lines.push("    bodyReader, err = client.encodeJSONBody(body)".into());
                    lines.push("    if err != nil {".into());
                    lines.push("        return nil, err".into());
                    lines.push("    }".into());
                    lines.push("}".into());
                }
            }
            RequestBodyKind::Multipart => {
                lines.push("var contentType string".into());
                if request_body.required {
                    if let Some(content_encoding) = content_encoding_attribute(&request_body.attributes) {
                        lines.push(format!(
                            "if err := client.validateStringEncoding(body, {:?}, \"request body\", requestOptions); err != nil {{",
                            content_encoding
                        ));
                        lines.push("    return nil, err".into());
                        lines.push("}".into());
                    }
                    lines.push(
                        "bodyReader, contentType, err = client.encodeMultipartBody(body)".into(),
                    );
                    lines.push("if err != nil {".into());
                    lines.push("    return nil, err".into());
                    lines.push("}".into());
                    lines.push("headers.Set(\"Content-Type\", contentType)".into());
                } else {
                    lines.push("if body != nil {".into());
                    if let Some(content_encoding) = content_encoding_attribute(&request_body.attributes) {
                        lines.push(format!(
                            "    if err := client.validateStringEncoding(body, {:?}, \"request body\", requestOptions); err != nil {{",
                            content_encoding
                        ));
                        lines.push("        return nil, err".into());
                        lines.push("    }".into());
                    }
                    lines.push(
                        "    bodyReader, contentType, err = client.encodeMultipartBody(body)"
                            .into(),
                    );
                    lines.push("    if err != nil {".into());
                    lines.push("        return nil, err".into());
                    lines.push("    }".into());
                    lines.push("    headers.Set(\"Content-Type\", contentType)".into());
                    lines.push("}".into());
                }
            }
            RequestBodyKind::BinaryOrOther => {
                if request_body.required {
                    lines.push("bodyReader = body".into());
                } else {
                    lines.push("bodyReader = body".into());
                }
            }
        }
    }
    lines.push("headers = client.mergeHeaders(headers, requestOptions)".into());
    lines.push(format!(
        "request, err := http.NewRequestWithContext(client.resolveContext(ctx, requestOptions), {}, client.buildURL(path, query), bodyReader)",
        go_http_method(operation.method)
    ));
    lines.push("if err != nil {".into());
    lines.push("    return nil, err".into());
    lines.push("}".into());
    lines.push("request.Header = headers".into());
    lines.push("for _, cookie := range cookies {".into());
    lines.push("    request.AddCookie(cookie)".into());
    lines.push("}".into());
    lines.push("return client.httpClient.Do(request)".into());

    indent_block(&lines, 4)
}

fn content_encoding_attribute(attributes: &BTreeMap<String, Value>) -> Option<&str> {
    attributes.get("content_encoding").and_then(Value::as_str)
}

#[derive(Debug, Clone, Copy)]
enum RequestBodyKind {
    Json,
    Multipart,
    BinaryOrOther,
}

fn classify_request_body(request_body: &RequestBody) -> RequestBodyKind {
    if request_body.media_type == "application/json" {
        RequestBodyKind::Json
    } else if request_body.media_type.starts_with("multipart/form-data") {
        RequestBodyKind::Multipart
    } else {
        RequestBodyKind::BinaryOrOther
    }
}

fn render_go_path_line(path: &str, params: &[arvalez_ir::Parameter]) -> String {
    let path_params = params
        .iter()
        .filter(|param| matches!(param.location, ParameterLocation::Path))
        .collect::<Vec<_>>();

    if path_params.is_empty() {
        format!("path := {:?}", path)
    } else {
        let mut format_path = String::new();
        let mut chars = path.chars().peekable();
        while let Some(ch) = chars.next() {
            match ch {
                '{' => {
                    while let Some(next) = chars.peek() {
                        if *next == '}' {
                            chars.next();
                            break;
                        }
                        chars.next();
                    }
                    format_path.push_str("%s");
                }
                '%' => format_path.push_str("%%"),
                _ => format_path.push(ch),
            }
        }
        let arguments = path_params
            .into_iter()
            .map(|param| {
                format!(
                    "url.PathEscape(fmt.Sprint({}))",
                    sanitize_identifier(&param.name)
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!("path := fmt.Sprintf({format_path:?}, {arguments})")
    }
}

fn render_model_block(tera: &Tera, model: ModelView) -> Result<String> {
    let mut context = TeraContext::new();
    context.insert("model", &model);
    tera.render(TEMPLATE_MODEL_STRUCT, &context)
        .context("failed to render model struct partial")
}

fn render_service_block(tera: &Tera, service: Result<ServiceView>) -> Result<String> {
    let service = service?;
    let mut context = TeraContext::new();
    context.insert("service", &service);
    tera.render(TEMPLATE_SERVICE, &context)
        .context("failed to render service partial")
}

fn render_method_blocks(tera: &Tera, methods: Vec<OperationMethodView>) -> Result<String> {
    methods
        .into_iter()
        .map(|method| {
            let mut context = TeraContext::new();
            context.insert("operation", &method);
            tera.render(TEMPLATE_CLIENT_METHOD, &context)
                .context("failed to render client method partial")
        })
        .collect::<Result<Vec<_>>>()
        .map(|blocks| blocks.join("\n\n"))
}

fn build_method_args(operation: &Operation) -> Vec<String> {
    let mut args = vec!["ctx context.Context".into()];

    for param in operation.params.iter().filter(|param| param.required) {
        args.push(format!(
            "{} {}",
            sanitize_identifier(&param.name),
            go_required_arg_type(&param.type_ref)
        ));
    }

    if let Some(request_body) = &operation.request_body
        && request_body.required
    {
        args.push(format!("body {}", go_body_arg_type(request_body, true)));
    }

    for param in operation.params.iter().filter(|param| !param.required) {
        args.push(format!(
            "{} {}",
            sanitize_identifier(&param.name),
            go_optional_arg_type(&param.type_ref)
        ));
    }

    if let Some(request_body) = &operation.request_body
        && !request_body.required
    {
        args.push(format!("body {}", go_body_arg_type(request_body, false)));
    }

    args.push("requestOptions *RequestOptions".into());
    args
}

fn build_forward_arguments(operation: &Operation) -> String {
    let mut args = vec!["ctx".into()];

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



fn go_field_type(type_ref: &TypeRef, optional: bool, nullable: bool) -> String {
    let base = go_type_ref(type_ref);
    if optional || nullable {
        match type_ref {
            TypeRef::Primitive { name } if name == "string" => "*string".into(),
            TypeRef::Primitive { name } if name == "integer" => "*int64".into(),
            TypeRef::Primitive { name } if name == "number" => "*float64".into(),
            TypeRef::Primitive { name } if name == "boolean" => "*bool".into(),
            TypeRef::Named { .. } => format!("*{base}"),
            _ => base,
        }
    } else {
        base
    }
}

fn go_type_ref(type_ref: &TypeRef) -> String {
    match type_ref {
        TypeRef::Primitive { name } => match name.as_str() {
            "string" => "string".into(),
            "integer" => "int64".into(),
            "number" => "float64".into(),
            "boolean" => "bool".into(),
            "binary" => "[]byte".into(),
            "null" => "any".into(),
            "any" | "object" => "any".into(),
            _ => "any".into(),
        },
        TypeRef::Named { name } => sanitize_exported_identifier(name),
        TypeRef::Array { item } => format!("[]{}", go_type_ref(item)),
        TypeRef::Map { value } => format!("map[string]{}", go_type_ref(value)),
        TypeRef::Union { .. } => "any".into(),
    }
}

fn go_body_arg_type(request_body: &RequestBody, required: bool) -> String {
    match request_body.type_ref.as_ref() {
        Some(TypeRef::Named { name }) => format!("*{}", sanitize_exported_identifier(name)),
        Some(type_ref) => {
            let base = go_type_ref(type_ref);
            if required {
                base
            } else {
                match type_ref {
                    TypeRef::Primitive { name } if name == "string" => "*string".into(),
                    TypeRef::Primitive { name } if name == "integer" => "*int64".into(),
                    TypeRef::Primitive { name } if name == "number" => "*float64".into(),
                    TypeRef::Primitive { name } if name == "boolean" => "*bool".into(),
                    _ => base,
                }
            }
        }
        None => "io.Reader".into(),
    }
}

fn go_required_arg_type(type_ref: &TypeRef) -> String {
    go_type_ref(type_ref)
}

fn go_optional_arg_type(type_ref: &TypeRef) -> String {
    match type_ref {
        TypeRef::Primitive { name } if name == "string" => "*string".into(),
        TypeRef::Primitive { name } if name == "integer" => "*int64".into(),
        TypeRef::Primitive { name } if name == "number" => "*float64".into(),
        TypeRef::Primitive { name } if name == "boolean" => "*bool".into(),
        TypeRef::Named { name } => format!("*{}", sanitize_exported_identifier(name)),
        _ => go_type_ref(type_ref),
    }
}

fn go_result_type(type_ref: &TypeRef) -> String {
    if returns_pointer_result(type_ref) {
        format!("*{}", go_decode_type(type_ref))
    } else {
        go_decode_type(type_ref)
    }
}

fn go_decode_type(type_ref: &TypeRef) -> String {
    match type_ref {
        TypeRef::Named { name } => sanitize_exported_identifier(name),
        _ => go_type_ref(type_ref),
    }
}

fn returns_pointer_result(type_ref: &TypeRef) -> bool {
    matches!(type_ref, TypeRef::Named { .. })
}

fn returns_nil_on_error(type_ref: &TypeRef) -> bool {
    match type_ref {
        TypeRef::Named { .. } | TypeRef::Array { .. } | TypeRef::Map { .. } => true,
        TypeRef::Primitive { name } => name == "binary",
        TypeRef::Union { .. } => false,
    }
}

fn go_http_method(method: HttpMethod) -> &'static str {
    match method {
        HttpMethod::Get => "http.MethodGet",
        HttpMethod::Post => "http.MethodPost",
        HttpMethod::Put => "http.MethodPut",
        HttpMethod::Patch => "http.MethodPatch",
        HttpMethod::Delete => "http.MethodDelete",
    }
}

fn default_package_name(module_path: &str) -> String {
    module_path
        .rsplit('/')
        .next()
        .map(sanitize_package_name)
        .unwrap_or_else(|| "client".into())
}

fn sanitize_package_name(name: &str) -> String {
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

fn sanitize_exported_identifier(name: &str) -> String {
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

fn sanitize_identifier(name: &str) -> String {
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

fn is_go_keyword(value: &str) -> bool {
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

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use arvalez_ir::{Attributes, Field, Parameter, Response};
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
                        attributes: BTreeMap::from([(
                            "description".into(),
                            Value::String("Unique widget identifier.".into()),
                        )]),
                    },
                    Parameter {
                        name: "include_count".into(),
                        location: ParameterLocation::Query,
                        type_ref: TypeRef::primitive("boolean"),
                        required: false,
                        attributes: BTreeMap::new(),
                    },
                ],
                request_body: Some(RequestBody {
                    required: false,
                    media_type: "application/json".into(),
                    type_ref: Some(TypeRef::named("Widget")),
                    attributes: BTreeMap::new(),
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
    fn renders_basic_go_package() {
        let files = generate_go_package(
            &sample_ir(),
            &GoPackageConfig::new("github.com/demo/client"),
        )
        .expect("package should render");

        let go_mod = files
            .iter()
            .find(|file| file.path.ends_with("go.mod"))
            .expect("go.mod");
        let models = files
            .iter()
            .find(|file| file.path.ends_with("models.go"))
            .expect("models.go");
        let client = files
            .iter()
            .find(|file| file.path.ends_with("client.go"))
            .expect("client.go");

        assert!(go_mod.contents.contains("module github.com/demo/client"));
        assert!(models.contents.contains("type Widget struct"));
        assert!(
            models
                .contents
                .contains("Count *int64 `json:\"count,omitempty\"`")
        );
        assert!(
            client
                .contents
                .contains("type ErrorHandler func(*http.Response) error")
        );
        assert!(client.contents.contains("type RequestOptions struct"));
        assert!(client.contents.contains("func (c *Client) GetWidgetRaw("));
        assert!(client.contents.contains("func (c *Client) GetWidget("));
        assert!(
            client
                .contents
                .contains("GetWidget parameter widgetId: Unique widget identifier.")
        );
        assert!(client.contents.contains("requestOptions *RequestOptions"));
        assert!(
            client
                .contents
                .contains("if err := client.handleError(response, requestOptions); err != nil {")
        );
        assert!(client.contents.contains("response, err := c.GetWidgetRaw("));
    }

    #[test]
    fn groups_operations_by_tag_when_enabled() {
        let files = generate_go_package(
            &sample_ir(),
            &GoPackageConfig::new("github.com/demo/client").with_group_by_tag(true),
        )
        .expect("package should render");
        let client = files
            .iter()
            .find(|file| file.path.ends_with("client.go"))
            .expect("client.go");

        assert!(client.contents.contains("Widgets *WidgetsService"));
        assert!(
            client
                .contents
                .contains("client.Widgets = &WidgetsService{client: client}")
        );
        assert!(client.contents.contains("type WidgetsService struct"));
        assert!(
            client
                .contents
                .contains("func (s *WidgetsService) GetWidgetRaw(")
        );
    }

    #[test]
    fn supports_selective_template_overrides() {
        let tempdir = tempdir().expect("tempdir");
        let partial_dir = tempdir.path().join("partials");
        fs::create_dir_all(&partial_dir).expect("partials dir");
        fs::write(
            partial_dir.join("service.go.tera"),
            "type {{ service.struct_name }} struct { Overridden bool }\n",
        )
        .expect("override template");

        let files = generate_go_package(
            &sample_ir(),
            &GoPackageConfig::new("github.com/demo/client")
                .with_group_by_tag(true)
                .with_template_dir(Some(tempdir.path().to_path_buf())),
        )
        .expect("package should render");
        let client = files
            .iter()
            .find(|file| file.path.ends_with("client.go"))
            .expect("client.go");

        assert!(
            client
                .contents
                .contains("type WidgetsService struct { Overridden bool }")
        );
    }
}
