use std::collections::BTreeMap;

use anyhow::{Context, Result};
use arvalez_ir::{CoreIr, Operation, Parameter, ParameterLocation, TypeRef};
use arvalez_target_core::{ClientLayout as SharedClientLayout, indent_block, sorted_models};
use serde::Serialize;
use serde_json::{Value, from_value};
use tera::{Context as TeraContext, Tera};

use super::{TEMPLATE_CLIENT_METHOD, TEMPLATE_MODEL_CLASS};
use crate::config::PythonPackageConfig;
use crate::sanitize::*;
use crate::types::*;

#[derive(Debug, Serialize)]
pub(crate) struct PackageTemplateContext {
    package_name: String,
    project_name: String,
    version: String,
    models: Vec<ModelView>,
    clients: Vec<ClientClassView>,
    tag_clients: Vec<TagClientClassView>,
}

impl PackageTemplateContext {
    pub(crate) fn from_ir(ir: &CoreIr, config: &PythonPackageConfig) -> Self {
        let models = sorted_models(ir)
            .into_iter()
            .map(ModelView::from_model)
            .collect::<Vec<_>>();

        let client_layout = ClientLayout::from_ir(ir);
        let mut tag_clients = Vec::new();

        if config.group_by_tag {
            for tag_group in &client_layout.tag_groups {
                tag_clients.push(TagClientClassView::async_client(tag_group));
                tag_clients.push(TagClientClassView::sync_client(tag_group));
            }
        }

        let operations = if config.group_by_tag {
            &client_layout.untagged_operations
        } else {
            &client_layout.all_operations
        };
        let tag_groups: &[TagGroup<'_>] = if config.group_by_tag {
            &client_layout.tag_groups
        } else {
            &[]
        };

        let clients = vec![
            ClientClassView::async_client(operations, tag_groups),
            ClientClassView::sync_client(operations, tag_groups),
        ];

        Self {
            package_name: config.package_name.clone(),
            project_name: config.project_name.clone(),
            version: config.version.clone(),
            models,
            clients,
            tag_clients,
        }
    }
}

#[derive(Debug)]
pub(crate) struct ClientLayout<'a> {
    pub(crate) all_operations: Vec<&'a Operation>,
    pub(crate) untagged_operations: Vec<&'a Operation>,
    pub(crate) tag_groups: Vec<TagGroup<'a>>,
}

impl<'a> ClientLayout<'a> {
    pub(crate) fn from_ir(ir: &'a CoreIr) -> Self {
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
pub(crate) struct TagGroup<'a> {
    pub(crate) property_name: String,
    pub(crate) class_base_name: String,
    pub(crate) operations: Vec<&'a Operation>,
}

impl<'a> TagGroup<'a> {
    pub(crate) fn new(tag: String, operations: Vec<&'a Operation>) -> Self {
        Self {
            property_name: sanitize_identifier(&tag),
            class_base_name: sanitize_class_name(&tag),
            operations,
        }
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct ModelView {
    class_name: String,
    has_fields: bool,
    is_enum: bool,
    is_alias: bool,
    enum_base_classes: String,
    enum_members_block: String,
    alias_expression: String,
    fields_block: String,
}

impl ModelView {
    pub(crate) fn from_model(model: &arvalez_ir::Model) -> Self {
        let field_lines = model
            .fields
            .iter()
            .map(ModelFieldView::from_field)
            .map(|field| field.declaration)
            .collect::<Vec<_>>();
        let enum_members = model
            .attributes
            .get("enum_values")
            .and_then(|value| value.as_array())
            .map(|values| {
                values
                    .iter()
                    .enumerate()
                    .map(|(index, value)| render_enum_member(value, index))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let is_enum = !enum_members.is_empty();
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

        Self {
            class_name: sanitize_class_name(&model.name),
            is_enum,
            is_alias,
            enum_base_classes: enum_base_classes(model),
            enum_members_block: indent_block(&enum_members, 4),
            alias_expression: alias_type_ref
                .map(|type_ref| python_field_type(&type_ref, false, alias_nullable))
                .unwrap_or_default(),
            has_fields: !field_lines.is_empty(),
            fields_block: indent_block(&field_lines, 4),
        }
    }
}

#[derive(Debug)]
pub(crate) struct ModelFieldView {
    declaration: String,
}

impl ModelFieldView {
    pub(crate) fn from_field(field: &arvalez_ir::Field) -> Self {
        let python_name = sanitize_identifier(&field.name);
        let type_hint = python_field_type(&field.type_ref, field.optional, field.nullable);
        let default_value = field.optional.then_some("None");
        let declaration = if python_name == field.name {
            match default_value {
                Some(default_value) => format!("{python_name}: {type_hint} = {default_value}"),
                None => format!("{python_name}: {type_hint}"),
            }
        } else {
            match default_value {
                Some(default_value) => format!(
                    "{python_name}: {type_hint} = Field(default={default_value}, alias={:?})",
                    field.name
                ),
                None => format!("{python_name}: {type_hint} = Field(alias={:?})", field.name),
            }
        };

        Self { declaration }
    }
}

/// A service sub-client binding, e.g. `self.widgets = AsyncWidgetsApi(self)`.
#[derive(Debug, Serialize)]
struct ServiceBindingView {
    property_name: String,
    class_name: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct ClientClassView {
    class_name: String,
    client_type: String,
    is_async: bool,
    service_bindings: Vec<ServiceBindingView>,
    methods: Vec<OperationMethodView>,
}

impl ClientClassView {
    pub(crate) fn async_client(
        operations: &[&Operation],
        tag_groups: &[TagGroup<'_>],
    ) -> Self {
        Self {
            class_name: "AsyncApiClient".into(),
            client_type: "httpx.AsyncClient".into(),
            is_async: true,
            service_bindings: tag_groups
                .iter()
                .map(|group| ServiceBindingView {
                    property_name: group.property_name.clone(),
                    class_name: format!("Async{}Api", group.class_base_name),
                })
                .collect(),
            methods: operations
                .iter()
                .map(|op| OperationMethodView::from_operation(op, ClientMode::Async))
                .collect(),
        }
    }

    pub(crate) fn sync_client(
        operations: &[&Operation],
        tag_groups: &[TagGroup<'_>],
    ) -> Self {
        Self {
            class_name: "SyncApiClient".into(),
            client_type: "httpx.Client".into(),
            is_async: false,
            service_bindings: tag_groups
                .iter()
                .map(|group| ServiceBindingView {
                    property_name: group.property_name.clone(),
                    class_name: format!("Sync{}Api", group.class_base_name),
                })
                .collect(),
            methods: operations
                .iter()
                .map(|op| OperationMethodView::from_operation(op, ClientMode::Sync))
                .collect(),
        }
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct TagClientClassView {
    class_name: String,
    owner_class_name: String,
    methods: Vec<OperationMethodView>,
}

impl TagClientClassView {
    pub(crate) fn async_client(tag_group: &TagGroup<'_>) -> Self {
        Self {
            class_name: format!("Async{}Api", tag_group.class_base_name),
            owner_class_name: "AsyncApiClient".into(),
            methods: tag_group
                .operations
                .iter()
                .map(|op| OperationMethodView::from_operation(op, ClientMode::Async))
                .collect(),
        }
    }

    pub(crate) fn sync_client(tag_group: &TagGroup<'_>) -> Self {
        Self {
            class_name: format!("Sync{}Api", tag_group.class_base_name),
            owner_class_name: "SyncApiClient".into(),
            methods: tag_group
                .operations
                .iter()
                .map(|op| OperationMethodView::from_operation(op, ClientMode::Sync))
                .collect(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum ClientMode {
    Async,
    Sync,
}

/// Documentation view for a single parameter.
#[derive(Debug, Serialize)]
struct PyDocParamView {
    name: String,
    description: String,
}

/// Per-parameter view for query / header / path parameters.
#[derive(Debug, Serialize)]
struct PyParamView {
    name: String,
    raw_name: String,
    required: bool,
    content_encoding: Option<String>,
}

/// Request-body descriptor.
#[derive(Debug, Serialize)]
struct PyBodyView {
    required: bool,
    /// `"json"` or `"data"` — the httpx keyword argument name.
    kind: String,
    content_encoding: Option<String>,
}

/// Return-type descriptor — drives both the method annotation and the
/// post-request parse/validation logic in the template.
#[derive(Debug, Serialize)]
struct PyReturnTypeView {
    /// Python return-type annotation, e.g. `"models.Widget"` or `"None"`.
    annotation: String,
    /// `true` when the operation has a typed success response.
    has_result: bool,
    /// Tera-safe expression passed to `_parse_response`, e.g. `"models.Widget"`.
    parse_expression: Option<String>,
    content_encoding: Option<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct OperationMethodView {
    is_async: bool,
    method_name: String,
    raw_method_name: String,
    args_signature: String,
    /// Pre-built keyword-argument list for forwarding from the wrapper to the raw method.
    forward_args: String,
    /// Operation summary text (newlines collapsed), if any.
    summary: Option<String>,
    /// Parameters that carry a non-empty description.
    doc_params: Vec<PyDocParamView>,
    /// HTTP method literal, e.g. `"GET"`.
    http_method: String,
    /// Path with `{param}` → `{sanitised_param}` (for use inside an f-string).
    path_fstring: String,
    path_params: Vec<PyParamView>,
    query_params: Vec<PyParamView>,
    header_params: Vec<PyParamView>,
    body: Option<PyBodyView>,
    return_type: PyReturnTypeView,
}

impl OperationMethodView {
    pub(crate) fn from_operation(operation: &Operation, mode: ClientMode) -> Self {
        let path_params: Vec<PyParamView> = operation
            .params
            .iter()
            .filter(|p| matches!(p.location, ParameterLocation::Path))
            .map(py_param_view)
            .collect();
        let query_params: Vec<PyParamView> = operation
            .params
            .iter()
            .filter(|p| matches!(p.location, ParameterLocation::Query))
            .map(py_param_view)
            .collect();
        let header_params: Vec<PyParamView> = operation
            .params
            .iter()
            .filter(|p| matches!(p.location, ParameterLocation::Header))
            .map(py_param_view)
            .collect();
        let body = operation.request_body.as_ref().map(|rb| PyBodyView {
            required: rb.required,
            kind: if rb.media_type == "application/json" {
                "json"
            } else {
                "data"
            }
            .into(),
            content_encoding: content_encoding_attribute(&rb.attributes).map(str::to_owned),
        });
        let ret = operation_return_type(operation);
        let content_encoding = operation
            .responses
            .iter()
            .find(|r| r.status.starts_with('2'))
            .and_then(|r| content_encoding_attribute(&r.attributes))
            .map(str::to_owned);
        let return_type = PyReturnTypeView {
            annotation: ret.annotation.unwrap_or_else(|| "None".into()),
            has_result: ret.parse_expression.is_some(),
            parse_expression: ret.parse_expression,
            content_encoding,
        };
        Self {
            is_async: matches!(mode, ClientMode::Async),
            method_name: sanitize_identifier(&operation.name),
            raw_method_name: raw_method_name(operation),
            args_signature: build_method_args(operation).join(", "),
            forward_args: build_wrapper_forward_arguments(operation),
            summary: operation
                .attributes
                .get("summary")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(sanitize_doc_text),
            doc_params: operation
                .params
                .iter()
                .filter_map(|param| {
                    param
                        .attributes
                        .get("description")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|d| !d.is_empty())
                        .map(|d| PyDocParamView {
                            name: sanitize_identifier(&param.name),
                            description: sanitize_doc_text(d),
                        })
                })
                .collect(),
            http_method: method_literal(operation.method).to_owned(),
            path_fstring: build_python_path_fstring(&operation.path),
            path_params,
            query_params,
            header_params,
            body,
            return_type,
        }
    }
}

pub(crate) fn build_wrapper_forward_arguments(operation: &Operation) -> String {
    let mut arguments = Vec::new();

    for param in operation.params.iter().filter(|param| param.required) {
        let name = sanitize_identifier(&param.name);
        arguments.push(format!("{name}={name}"));
    }

    if let Some(request_body) = &operation.request_body
        && request_body.required
    {
        let _ = request_body;
        arguments.push("body=body".into());
    }

    for param in operation.params.iter().filter(|param| !param.required) {
        let name = sanitize_identifier(&param.name);
        arguments.push(format!("{name}={name}"));
    }

    if let Some(request_body) = &operation.request_body
        && !request_body.required
    {
        let _ = request_body;
        arguments.push("body=body".into());
    }

    arguments.push("request_options=request_options".into());
    arguments.join(", ")
}

pub(crate) fn build_method_args(operation: &Operation) -> Vec<String> {
    let mut args = Vec::new();

    for param in operation.params.iter().filter(|param| param.required) {
        args.push(format!(
            "{}: {}",
            sanitize_identifier(&param.name),
            python_type_ref(&param.type_ref, PythonContext::Client)
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
                .map(|type_ref| python_type_ref(type_ref, PythonContext::Client))
                .unwrap_or_else(|| "Any".into())
        ));
    }

    for param in operation.params.iter().filter(|param| !param.required) {
        args.push(format!(
            "{}: {} | None = None",
            sanitize_identifier(&param.name),
            python_type_ref(&param.type_ref, PythonContext::Client)
        ));
    }

    if let Some(request_body) = &operation.request_body
        && !request_body.required
    {
        args.push(format!(
            "body: {} | None = None",
            request_body
                .type_ref
                .as_ref()
                .map(|type_ref| python_type_ref(type_ref, PythonContext::Client))
                .unwrap_or_else(|| "Any".into())
        ));
    }

    args.push("request_options: RequestOptions | None = None".into());

    args
}

fn build_python_path_fstring(path: &str) -> String {
    let mut result = String::new();
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
                result.push('{');
                result.push_str(&sanitize_identifier(&name));
                result.push('}');
            }
            '"' => result.push_str("\\\""),
            _ => result.push(ch),
        }
    }
    result
}

fn py_param_view(param: &Parameter) -> PyParamView {
    PyParamView {
        name: sanitize_identifier(&param.name),
        raw_name: param.name.clone(),
        required: param.required,
        content_encoding: content_encoding_attribute(&param.attributes).map(str::to_owned),
    }
}

pub(crate) fn sanitize_doc_text(value: &str) -> String {
    value.replace("\"\"\"", "\"\\\"\\\"")
}

pub(crate) fn content_encoding_attribute(attributes: &BTreeMap<String, Value>) -> Option<&str> {
    attributes.get("content_encoding").and_then(Value::as_str)
}

pub(crate) fn raw_method_name(operation: &Operation) -> String {
    format!("_{}_raw", sanitize_identifier(&operation.name))
}

pub(crate) fn render_model_block(tera: &Tera, model: ModelView) -> Result<String> {
    let mut context = TeraContext::new();
    context.insert("model", &model);
    tera.render(TEMPLATE_MODEL_CLASS, &context)
        .context("failed to render model class partial")
}

// ── IrEmitter helpers ───────────────────────────────────────────────────────────

/// Render a single model to a Python dataclass declaration.
pub(crate) fn emit_model(tera: &Tera, model: &arvalez_ir::Model) -> Result<String> {
    render_model_block(tera, ModelView::from_model(model))
}

/// Render a single operation to a Python async client method.
pub(crate) fn emit_operation(
    tera: &Tera,
    operation: &arvalez_ir::Operation,
) -> Result<String> {
    render_method_block(tera, OperationMethodView::from_operation(operation, ClientMode::Async))
}

pub(crate) fn render_method_block(tera: &Tera, method: OperationMethodView) -> Result<String> {
    let mut context = TeraContext::new();
    context.insert("operation", &method);
    tera.render(TEMPLATE_CLIENT_METHOD, &context)
        .context("failed to render client method partial")
}
