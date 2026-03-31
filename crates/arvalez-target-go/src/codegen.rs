use std::collections::BTreeMap;

use anyhow::{Context, Result};
use arvalez_ir::{CoreIr, Field, Model, Operation, Parameter, ParameterLocation, RequestBody};
use serde::Serialize;
use serde_json::Value;
use tera::{Context as TeraContext, Tera};

use arvalez_target_core::{ClientLayout as SharedClientLayout, indent_block, sorted_models};

use super::{GoPackageConfig, TEMPLATE_CLIENT_METHOD, TEMPLATE_MODEL_STRUCT, TEMPLATE_SERVICE};
use crate::sanitize::{sanitize_exported_identifier, sanitize_identifier};
use crate::types::{
    go_body_arg_type, go_decode_type, go_field_type, go_http_method, go_optional_arg_type,
    go_required_arg_type, go_result_type, returns_nil_on_error, returns_pointer_result,
};

#[derive(Debug, Serialize)]
pub(crate) struct PackageTemplateContext {
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
    pub(crate) fn from_ir(ir: &CoreIr, config: &GoPackageConfig, tera: &Tera) -> Result<Self> {
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
    fn from_model(model: &Model) -> Self {
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
    fn from_field(field: &Field) -> Self {
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

fn render_go_path_line(path: &str, params: &[Parameter]) -> String {
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
