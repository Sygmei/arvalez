use std::collections::BTreeMap;

use anyhow::{Context, Result};
use arvalez_ir::{CoreIr, Operation, ParameterLocation};
use arvalez_target_core::{ClientLayout, indent_block, sorted_models, sorted_operations};
use serde::Serialize;
use serde_json::{Value, from_value};
use tera::{Context as TeraContext, Tera};

use super::{
    TEMPLATE_CLIENT_METHOD, TEMPLATE_MODEL_INTERFACE, TEMPLATE_TAG_GROUP,
};
use crate::config::TypeScriptPackageConfig;
use crate::sanitize::*;
use crate::types::*;

#[derive(Debug, Serialize)]
pub(crate) struct PackageTemplateContext {
    package_name: String,
    version: String,
    client_imports: String,
    model_blocks: Vec<String>,
    tag_group_blocks: Vec<String>,
    method_blocks: Vec<String>,
    index_model_exports: String,
}

impl PackageTemplateContext {
    pub(crate) fn from_ir(ir: &CoreIr, config: &TypeScriptPackageConfig, tera: &Tera) -> Result<Self> {
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
        use arvalez_ir::TypeRef;

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

fn sorted_model_names(ir: &CoreIr) -> Vec<String> {
    sorted_models(ir)
        .into_iter()
        .map(|model| model.name.clone())
        .collect()
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
