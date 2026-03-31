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

/// Documentation view for a single parameter — carries the sanitised name and
/// description so the template can render Go `//` comment lines without any
/// Rust-side string pre-building.
#[derive(Debug, Serialize)]
struct GoDocParamView {
    name: String,
    description: String,
}

/// Per-parameter view: carries all data the template needs to emit
/// path / query / header / cookie handling without Rust-side Go-code pre-building.
#[derive(Debug, Serialize)]
struct GoParamView {
    /// Sanitised Go identifier used as the variable name in generated code.
    name: String,
    /// Original parameter name (map key / header name / cookie name / URL key).
    raw_name: String,
    /// `true` = non-pointer arg in the Go signature; `false` = pointer arg.
    required: bool,
    /// Optional `content_encoding` attribute value, e.g. `"base64"`.
    content_encoding: Option<String>,
}

fn go_param_view(param: &Parameter) -> GoParamView {
    GoParamView {
        name: sanitize_identifier(&param.name),
        raw_name: param.name.clone(),
        required: param.required,
        content_encoding: content_encoding_attribute(&param.attributes).map(str::to_owned),
    }
}

/// Request-body descriptor — tells the template how to encode the body.
#[derive(Debug, Serialize)]
struct GoBodyView {
    required: bool,
    /// `"json"`, `"multipart"`, or `"binary"`.
    kind: String,
    content_encoding: Option<String>,
}

/// Return-shape descriptor — tells the template how to handle the response.
#[derive(Debug, Serialize)]
struct GoReturnShapeView {
    /// `true` when the operation has a typed success response body.
    has_result: bool,
    /// Full Go return type, e.g. `"(*Widget, error)"` or `"error"`.
    signature: String,
    /// Outer result type (may be a pointer), e.g. `"*Widget"`.
    result_go_type: String,
    /// Concrete decode target (without outer pointer), e.g. `"Widget"`.
    decode_go_type: String,
    /// `true` when the result is a pointer / slice → `return nil, err` on error.
    returns_nil_on_error: bool,
    /// `true` when the success return is `&result` rather than `result`.
    returns_pointer: bool,
    /// Optional content-encoding to validate on the decoded response body.
    content_encoding: Option<String>,
}

impl GoReturnShapeView {
    fn from_operation(operation: &Operation) -> Self {
        let success = operation
            .responses
            .iter()
            .find(|r| r.status.starts_with('2'));
        let content_encoding = success
            .and_then(|r| content_encoding_attribute(&r.attributes))
            .map(str::to_owned);
        match success.and_then(|r| r.type_ref.as_ref()) {
            Some(type_ref) => {
                let result_go_type = go_result_type(type_ref);
                let decode_go_type = go_decode_type(type_ref);
                GoReturnShapeView {
                    has_result: true,
                    signature: format!("({result_go_type}, error)"),
                    returns_nil_on_error: returns_nil_on_error(type_ref),
                    returns_pointer: returns_pointer_result(type_ref),
                    decode_go_type,
                    result_go_type,
                    content_encoding,
                }
            }
            None => GoReturnShapeView {
                has_result: false,
                signature: "error".into(),
                result_go_type: String::new(),
                decode_go_type: String::new(),
                returns_nil_on_error: false,
                returns_pointer: false,
                content_encoding,
            },
        }
    }
}

/// Converts a path template such as `"/items/{id}/sub/{sub}"` into a Go
/// `fmt.Sprintf` format string by replacing `{param}` placeholders with `%s`
/// and escaping literal `%` characters as `%%`.
fn build_path_format(path: &str) -> String {
    let mut result = String::new();
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
                result.push_str("%s");
            }
            '%' => result.push_str("%%"),
            _ => result.push(ch),
        }
    }
    result
}

#[derive(Debug, Serialize)]
struct OperationMethodView {
    receiver_name: String,
    receiver_type: String,
    client_expression: String,
    method_name: String,
    raw_method_name: String,
    /// Pre-built Go argument list for the function signature.
    args_signature: String,
    /// Pre-built argument list for forwarding from wrapper to raw method.
    forward_args: String,
    /// Operation summary text (newlines collapsed), if any.
    summary: Option<String>,
    /// Parameters that carry a non-empty description.
    doc_params: Vec<GoDocParamView>,
    /// Go HTTP method constant, e.g. `"http.MethodGet"`.
    http_method: String,
    /// Path with `{param}` → `%s` and literal `%` → `%%` for `fmt.Sprintf`.
    path_format: String,
    /// Original path string from the IR, used when there are no path params.
    path_raw: String,
    path_params: Vec<GoParamView>,
    query_params: Vec<GoParamView>,
    header_params: Vec<GoParamView>,
    cookie_params: Vec<GoParamView>,
    body: Option<GoBodyView>,
    return_shape: GoReturnShapeView,
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
        let path_params: Vec<GoParamView> = operation
            .params
            .iter()
            .filter(|p| matches!(p.location, ParameterLocation::Path))
            .map(go_param_view)
            .collect();
        let query_params: Vec<GoParamView> = operation
            .params
            .iter()
            .filter(|p| matches!(p.location, ParameterLocation::Query))
            .map(go_param_view)
            .collect();
        let header_params: Vec<GoParamView> = operation
            .params
            .iter()
            .filter(|p| matches!(p.location, ParameterLocation::Header))
            .map(go_param_view)
            .collect();
        let cookie_params: Vec<GoParamView> = operation
            .params
            .iter()
            .filter(|p| matches!(p.location, ParameterLocation::Cookie))
            .map(go_param_view)
            .collect();
        let body = operation.request_body.as_ref().map(|rb| GoBodyView {
            required: rb.required,
            kind: match classify_request_body(rb) {
                RequestBodyKind::Json => "json".into(),
                RequestBodyKind::Multipart => "multipart".into(),
                RequestBodyKind::BinaryOrOther => "binary".into(),
            },
            content_encoding: content_encoding_attribute(&rb.attributes).map(str::to_owned),
        });
        let method_name = sanitize_exported_identifier(&operation.name);
        let raw_method_name = format!("{}Raw", &method_name);
        Self {
            receiver_name: receiver_name.into(),
            receiver_type: receiver_type.into(),
            client_expression: client_expression.into(),
            method_name,
            raw_method_name,
            args_signature: build_method_args(operation).join(", "),
            forward_args: build_forward_arguments(operation),
            summary: operation
                .attributes
                .get("summary")
                .and_then(Value::as_str)
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .map(sanitize_go_comment_text),
            doc_params: operation
                .params
                .iter()
                .filter_map(|param| {
                    param
                        .attributes
                        .get("description")
                        .and_then(Value::as_str)
                        .map(|d| d.trim())
                        .filter(|d| !d.is_empty())
                        .map(|d| GoDocParamView {
                            name: sanitize_identifier(&param.name),
                            description: sanitize_go_comment_text(d),
                        })
                })
                .collect(),
            http_method: go_http_method(operation.method).to_owned(),
            path_format: build_path_format(&operation.path),
            path_raw: operation.path.clone(),
            path_params,
            query_params,
            header_params,
            cookie_params,
            body,
            return_shape: GoReturnShapeView::from_operation(operation),
        }
    }
}

fn sanitize_go_comment_text(value: &str) -> String {
    value.replace('\n', " ").replace('\r', " ")
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



fn render_model_block(tera: &Tera, model: ModelView) -> Result<String> {
    let mut context = TeraContext::new();
    context.insert("model", &model);
    tera.render(TEMPLATE_MODEL_STRUCT, &context)
        .context("failed to render model struct partial")
}

// ── IrEmitter helpers ───────────────────────────────────────────────────────────

/// Render a single model to a Go struct declaration.
pub(crate) fn emit_model(tera: &Tera, model: &arvalez_ir::Model) -> Result<String> {
    render_model_block(tera, ModelView::from_model(model))
}

/// Render a single operation to a Go client method pair (raw + wrapper).
pub(crate) fn emit_operation(
    tera: &Tera,
    operation: &arvalez_ir::Operation,
) -> Result<String> {
    render_method_blocks(tera, vec![OperationMethodView::client_method(operation)])
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
