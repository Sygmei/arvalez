use std::collections::BTreeMap;

use anyhow::{Context, Result};
use arvalez_ir::{CoreIr, Operation, ParameterLocation, RequestBody, TypeRef};
use arvalez_target_core::{ClientLayout as SharedClientLayout, indent_block, sorted_models};
use serde::Serialize;
use serde_json::{Value, from_value};
use tera::{Context as TeraContext, Tera};

use super::{
    TEMPLATE_CLIENT_CLASS, TEMPLATE_CLIENT_METHOD, TEMPLATE_MODEL_CLASS, TEMPLATE_TAG_CLIENT_CLASS,
};
use crate::config::PythonPackageConfig;
use crate::sanitize::*;
use crate::types::*;

#[derive(Debug, Serialize)]
pub(crate) struct PackageTemplateContext {
    package_name: String,
    project_name: String,
    version: String,
    model_names: Vec<String>,
    model_imports_block: String,
    model_exports_block: String,
    model_blocks: Vec<String>,
    client_names: Vec<String>,
    client_blocks: Vec<String>,
}

impl PackageTemplateContext {
    pub(crate) fn from_ir(ir: &CoreIr, config: &PythonPackageConfig, tera: &Tera) -> Result<Self> {
        let model_names = sorted_models(ir)
            .into_iter()
            .map(|model| sanitize_class_name(&model.name))
            .collect::<Vec<_>>();
        let model_imports_block = indent_block(
            &model_names
                .iter()
                .map(|name| format!("{name},"))
                .collect::<Vec<_>>(),
            4,
        );
        let model_exports_block = indent_block(
            &model_names
                .iter()
                .map(|name| format!("{name:?},"))
                .collect::<Vec<_>>(),
            4,
        );

        let model_blocks = sorted_models(ir)
            .into_iter()
            .map(|model| render_model_block(tera, ModelView::from_model(model)))
            .collect::<Result<Vec<_>>>()?;

        let client_layout = ClientLayout::from_ir(ir);
        let mut client_blocks = Vec::new();

        if config.group_by_tag {
            for tag_group in &client_layout.tag_groups {
                client_blocks.push(render_tag_client_block(
                    tera,
                    TagClientClassView::async_client(tag_group, tera)?,
                )?);
                client_blocks.push(render_tag_client_block(
                    tera,
                    TagClientClassView::sync_client(tag_group, tera)?,
                )?);
            }
        }

        let async_client = render_client_block(
            tera,
            ClientClassView::async_client(
                if config.group_by_tag {
                    &client_layout.untagged_operations
                } else {
                    &client_layout.all_operations
                },
                if config.group_by_tag {
                    &client_layout.tag_groups
                } else {
                    &[]
                },
                tera,
            )?,
        )?;
        let sync_client = render_client_block(
            tera,
            ClientClassView::sync_client(
                if config.group_by_tag {
                    &client_layout.untagged_operations
                } else {
                    &client_layout.all_operations
                },
                if config.group_by_tag {
                    &client_layout.tag_groups
                } else {
                    &[]
                },
                tera,
            )?,
        )?;
        client_blocks.push(async_client);
        client_blocks.push(sync_client);

        Ok(Self {
            package_name: config.package_name.clone(),
            project_name: config.project_name.clone(),
            version: config.version.clone(),
            model_names,
            model_imports_block,
            model_exports_block,
            model_blocks,
            client_names: vec![
                "ApiClient".into(),
                "AsyncApiClient".into(),
                "SyncApiClient".into(),
            ],
            client_blocks,
        })
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

#[derive(Debug, Serialize)]
pub(crate) struct ClientClassView {
    class_name: String,
    client_type: String,
    service_bindings_block: String,
    close_method_signature: String,
    close_method_block: String,
    enter_method_signature: String,
    exit_method_signature: String,
    exit_method_block: String,
    methods_block: String,
}

impl ClientClassView {
    pub(crate) fn async_client(
        operations: &[&Operation],
        tag_groups: &[TagGroup<'_>],
        tera: &Tera,
    ) -> Result<Self> {
        let methods = operations
            .into_iter()
            .map(|operation| OperationMethodView::from_operation(operation, ClientMode::Async))
            .collect::<Vec<_>>();
        Ok(Self {
            class_name: "AsyncApiClient".into(),
            client_type: "httpx.AsyncClient".into(),
            service_bindings_block: indent_block(
                &tag_groups
                    .iter()
                    .map(|group| {
                        format!(
                            "self.{} = Async{}Api(self)",
                            group.property_name, group.class_base_name
                        )
                    })
                    .collect::<Vec<_>>(),
                8,
            ),
            close_method_signature: "async def aclose(self) -> None:".into(),
            close_method_block: indent_block(
                &[
                    "if self._owns_client:".into(),
                    "    await self._client.aclose()".into(),
                ],
                8,
            ),
            enter_method_signature: "async def __aenter__(self) -> AsyncApiClient:".into(),
            exit_method_signature:
                "async def __aexit__(self, exc_type: Any, exc: Any, tb: Any) -> None:".into(),
            exit_method_block: indent_block(&["await self.aclose()".into()], 8),
            methods_block: render_methods_block(tera, methods)?,
        })
    }

    pub(crate) fn sync_client(
        operations: &[&Operation],
        tag_groups: &[TagGroup<'_>],
        tera: &Tera,
    ) -> Result<Self> {
        let methods = operations
            .into_iter()
            .map(|operation| OperationMethodView::from_operation(operation, ClientMode::Sync))
            .collect::<Vec<_>>();
        Ok(Self {
            class_name: "SyncApiClient".into(),
            client_type: "httpx.Client".into(),
            service_bindings_block: indent_block(
                &tag_groups
                    .iter()
                    .map(|group| {
                        format!(
                            "self.{} = Sync{}Api(self)",
                            group.property_name, group.class_base_name
                        )
                    })
                    .collect::<Vec<_>>(),
                8,
            ),
            close_method_signature: "def close(self) -> None:".into(),
            close_method_block: indent_block(
                &[
                    "if self._owns_client:".into(),
                    "    self._client.close()".into(),
                ],
                8,
            ),
            enter_method_signature: "def __enter__(self) -> SyncApiClient:".into(),
            exit_method_signature: "def __exit__(self, exc_type: Any, exc: Any, tb: Any) -> None:"
                .into(),
            exit_method_block: indent_block(&["self.close()".into()], 8),
            methods_block: render_methods_block(tera, methods)?,
        })
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct TagClientClassView {
    class_name: String,
    owner_class_name: String,
    methods_block: String,
}

impl TagClientClassView {
    pub(crate) fn async_client(tag_group: &TagGroup<'_>, tera: &Tera) -> Result<Self> {
        let methods = tag_group
            .operations
            .iter()
            .map(|operation| OperationMethodView::from_operation(operation, ClientMode::Async))
            .collect::<Vec<_>>();

        Ok(Self {
            class_name: format!("Async{}Api", tag_group.class_base_name),
            owner_class_name: "AsyncApiClient".into(),
            methods_block: render_methods_block(tera, methods)?,
        })
    }

    pub(crate) fn sync_client(tag_group: &TagGroup<'_>, tera: &Tera) -> Result<Self> {
        let methods = tag_group
            .operations
            .iter()
            .map(|operation| OperationMethodView::from_operation(operation, ClientMode::Sync))
            .collect::<Vec<_>>();

        Ok(Self {
            class_name: format!("Sync{}Api", tag_group.class_base_name),
            owner_class_name: "SyncApiClient".into(),
            methods_block: render_methods_block(tera, methods)?,
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum ClientMode {
    Async,
    Sync,
}

#[derive(Debug, Serialize)]
pub(crate) struct OperationMethodView {
    def_keyword: String,
    method_name: String,
    raw_method_name: String,
    args_signature: String,
    return_annotation: String,
    raw_return_annotation: String,
    raw_docstring_block: String,
    docstring_block: String,
    url_template: String,
    raw_request_call_line: String,
    raw_pre_request_block: String,
    raw_post_request_block: String,
    wrapper_request_call_line: String,
    post_request_block: String,
}

impl OperationMethodView {
    pub(crate) fn from_operation(operation: &Operation, mode: ClientMode) -> Self {
        let mut pre_request_lines = Vec::new();
        let mut call_arguments = Vec::new();
        let mut params_name = "None".to_owned();
        let mut headers_name = "None".to_owned();

        for param in operation
            .params
            .iter()
            .filter(|param| matches!(param.location, ParameterLocation::Path))
        {
            if let Some(content_encoding) = content_encoding_attribute(&param.attributes) {
                let param_name = sanitize_identifier(&param.name);
                pre_request_lines.push(format!(
                    "self._validate_string_encoding({param_name}, {content_encoding:?}, {:?}, request_options)",
                    format!("path parameter `{}`", param.name)
                ));
            }
        }

        let query_params = operation
            .params
            .iter()
            .filter(|param| matches!(param.location, ParameterLocation::Query))
            .collect::<Vec<_>>();
        if !query_params.is_empty() {
            pre_request_lines.push("params: dict[str, Any] = {}".into());
            params_name = "params".into();
            for param in query_params {
                let param_name = sanitize_identifier(&param.name);
                if param.required {
                    if let Some(content_encoding) = content_encoding_attribute(&param.attributes) {
                        pre_request_lines.push(format!(
                            "self._validate_string_encoding({param_name}, {content_encoding:?}, {:?}, request_options)",
                            format!("query parameter `{}`", param.name)
                        ));
                    }
                    pre_request_lines.push(format!("params[{:?}] = {param_name}", param.name));
                } else {
                    pre_request_lines.push(format!("if {param_name} is not None:"));
                    if let Some(content_encoding) = content_encoding_attribute(&param.attributes) {
                        pre_request_lines.push(format!(
                            "    self._validate_string_encoding({param_name}, {content_encoding:?}, {:?}, request_options)",
                            format!("query parameter `{}`", param.name)
                        ));
                    }
                    pre_request_lines.push(format!("    params[{:?}] = {param_name}", param.name));
                }
            }
            call_arguments.push("params=params".into());
        }

        let header_params = operation
            .params
            .iter()
            .filter(|param| matches!(param.location, ParameterLocation::Header))
            .collect::<Vec<_>>();
        if !header_params.is_empty() {
            pre_request_lines.push("headers: dict[str, Any] = {}".into());
            headers_name = "headers".into();
            for param in header_params {
                let param_name = sanitize_identifier(&param.name);
                if param.required {
                    if let Some(content_encoding) = content_encoding_attribute(&param.attributes) {
                        pre_request_lines.push(format!(
                            "self._validate_string_encoding({param_name}, {content_encoding:?}, {:?}, request_options)",
                            format!("header `{}`", param.name)
                        ));
                    }
                    pre_request_lines.push(format!("headers[{:?}] = {param_name}", param.name));
                } else {
                    pre_request_lines.push(format!("if {param_name} is not None:"));
                    if let Some(content_encoding) = content_encoding_attribute(&param.attributes) {
                        pre_request_lines.push(format!(
                            "    self._validate_string_encoding({param_name}, {content_encoding:?}, {:?}, request_options)",
                            format!("header `{}`", param.name)
                        ));
                    }
                    pre_request_lines.push(format!("    headers[{:?}] = {param_name}", param.name));
                }
            }
            call_arguments.push("headers=headers".into());
        }

        let uses_request_kwargs = true;
        pre_request_lines.push("request_kwargs: dict[str, Any] = {}".into());
        if let Some(request_body) = &operation.request_body {
            if request_body.required {
                if let Some(content_encoding) = content_encoding_attribute(&request_body.attributes) {
                    pre_request_lines.push(format!(
                        "self._validate_string_encoding(body, {content_encoding:?}, \"request body\", request_options)"
                    ));
                }
                call_arguments.push(required_request_body_argument(request_body));
            } else {
                if let Some(content_encoding) = content_encoding_attribute(&request_body.attributes) {
                    pre_request_lines.push("if body is not None:".into());
                    pre_request_lines.push(format!(
                        "    self._validate_string_encoding(body, {content_encoding:?}, \"request body\", request_options)"
                    ));
                }
                pre_request_lines.extend(optional_request_body_lines(request_body));
            }
        }
        pre_request_lines.push(format!(
            "request_kwargs = self._apply_request_options(request_kwargs, request_options, params={params_name}, headers={headers_name})"
        ));

        let return_type = operation_return_type(operation);
        let mut post_request_lines = vec!["self._handle_error(response, request_options)".into()];
        let response_encoding = operation
            .responses
            .iter()
            .find(|response| response.status.starts_with('2'))
            .and_then(|response| content_encoding_attribute(&response.attributes));
        if let Some(parse_expression) = return_type.parse_expression.clone() {
            post_request_lines.push(format!(
                "return self._parse_response(response, {parse_expression}, response_encoding={}, request_options=request_options)",
                response_encoding
                    .map(|encoding| format!("{encoding:?}"))
                    .unwrap_or_else(|| "None".into())
            ));
        } else {
            post_request_lines.push(format!(
                "return self._parse_response(response, response_encoding={}, request_options=request_options)",
                response_encoding
                    .map(|encoding| format!("{encoding:?}"))
                    .unwrap_or_else(|| "None".into())
            ));
        }

        let request_suffix = render_request_arguments(&call_arguments, uses_request_kwargs);
        let raw_request_call_line = match mode {
            ClientMode::Async => format!(
                "response = await self._client.request({:?}, url{request_suffix})",
                method_literal(operation.method)
            ),
            ClientMode::Sync => format!(
                "response = self._client.request({:?}, url{request_suffix})",
                method_literal(operation.method)
            ),
        };
        let wrapper_request_call_line = match mode {
            ClientMode::Async => format!(
                "response = await self.{}({})",
                raw_method_name(operation),
                build_wrapper_forward_arguments(operation)
            ),
            ClientMode::Sync => format!(
                "response = self.{}({})",
                raw_method_name(operation),
                build_wrapper_forward_arguments(operation)
            ),
        };

        Self {
            def_keyword: match mode {
                ClientMode::Async => "async def".into(),
                ClientMode::Sync => "def".into(),
            },
            method_name: sanitize_identifier(&operation.name),
            raw_method_name: raw_method_name(operation),
            args_signature: build_method_args(operation).join(", "),
            return_annotation: return_type.annotation.unwrap_or_else(|| "None".into()),
            raw_return_annotation: "httpx.Response".into(),
            raw_docstring_block: render_python_method_docstring(operation, true),
            docstring_block: render_python_method_docstring(operation, false),
            url_template: render_python_path_template(&operation.path),
            raw_request_call_line,
            raw_pre_request_block: indent_block(&pre_request_lines, 8),
            raw_post_request_block: indent_block(&["return response".into()], 8),
            wrapper_request_call_line,
            post_request_block: indent_block(&post_request_lines, 8),
        }
    }
}

pub(crate) fn render_python_method_docstring(operation: &Operation, raw: bool) -> String {
    let mut lines = Vec::new();
    if let Some(summary) = operation.attributes.get("summary").and_then(Value::as_str) {
        let summary = summary.trim();
        if !summary.is_empty() {
            lines.push(summary.to_owned());
        }
    }
    if raw {
        lines.push("Returns the raw HTTP response without parsing it or raising for HTTP errors.".into());
    }

    let described_params = operation
        .params
        .iter()
        .filter_map(|param| {
            param.attributes
                .get("description")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|description| !description.is_empty())
                .map(|description| (sanitize_identifier(&param.name), description.to_owned()))
        })
        .collect::<Vec<_>>();

    if !described_params.is_empty() {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.push("Args:".into());
        for (name, description) in described_params {
            lines.push(format!("    {name}: {}", sanitize_doc_text(&description)));
        }
    }

    if lines.is_empty() {
        String::new()
    } else {
        let mut block = vec!["\"\"\"".to_owned()];
        block.extend(lines);
        block.push("\"\"\"".into());
        indent_block(&block, 8)
    }
}

pub(crate) fn sanitize_doc_text(value: &str) -> String {
    value.replace("\"\"\"", "\"\\\"\\\"")
}

pub(crate) fn content_encoding_attribute(attributes: &BTreeMap<String, Value>) -> Option<&str> {
    attributes.get("content_encoding").and_then(Value::as_str)
}

pub(crate) fn render_request_arguments(call_arguments: &[String], uses_request_kwargs: bool) -> String {
    let mut arguments = call_arguments.to_vec();
    if uses_request_kwargs {
        arguments.push("**request_kwargs".into());
    }
    if arguments.is_empty() {
        String::new()
    } else {
        format!(", {}", arguments.join(", "))
    }
}

pub(crate) fn required_request_body_argument(request_body: &RequestBody) -> String {
    let (body_kwarg, json_mode_literal) = request_body_binding(request_body);
    format!("{body_kwarg}=self._serialize_body(body, json_mode={json_mode_literal})")
}

pub(crate) fn optional_request_body_lines(request_body: &RequestBody) -> Vec<String> {
    let (body_kwarg, json_mode_literal) = request_body_binding(request_body);
    vec![
        "if body is not None:".into(),
        format!(
            "    request_kwargs[{body_kwarg:?}] = self._serialize_body(body, json_mode={json_mode_literal})"
        ),
    ]
}

pub(crate) fn request_body_binding(request_body: &RequestBody) -> (&'static str, &'static str) {
    let json_mode = request_body.media_type == "application/json";
    let json_mode_literal = if json_mode { "True" } else { "False" };
    let body_kwarg = if json_mode { "json" } else { "data" };
    (body_kwarg, json_mode_literal)
}

pub(crate) fn render_model_block(tera: &Tera, model: ModelView) -> Result<String> {
    let mut context = TeraContext::new();
    context.insert("model", &model);
    tera.render(TEMPLATE_MODEL_CLASS, &context)
        .context("failed to render model class partial")
}

pub(crate) fn render_client_block(tera: &Tera, client: ClientClassView) -> Result<String> {
    let mut context = TeraContext::new();
    context.insert("client", &client);
    tera.render(TEMPLATE_CLIENT_CLASS, &context)
        .context("failed to render client class partial")
}

pub(crate) fn render_tag_client_block(tera: &Tera, client: TagClientClassView) -> Result<String> {
    let mut context = TeraContext::new();
    context.insert("client", &client);
    tera.render(TEMPLATE_TAG_CLIENT_CLASS, &context)
        .context("failed to render tag client class partial")
}

pub(crate) fn render_methods_block(tera: &Tera, methods: Vec<OperationMethodView>) -> Result<String> {
    methods
        .into_iter()
        .map(|method| render_method_block(tera, method))
        .collect::<Result<Vec<_>>>()
        .map(|methods| methods.join("\n"))
}

pub(crate) fn render_method_block(tera: &Tera, method: OperationMethodView) -> Result<String> {
    let mut context = TeraContext::new();
    context.insert("operation", &method);
    tera.render(TEMPLATE_CLIENT_METHOD, &context)
        .context("failed to render client method partial")
}

pub(crate) fn raw_method_name(operation: &Operation) -> String {
    format!("_{}_raw", sanitize_identifier(&operation.name))
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
