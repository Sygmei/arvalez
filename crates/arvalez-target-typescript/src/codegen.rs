use std::collections::BTreeMap;

use anyhow::{Context, Result};
use arvalez_ir::{CoreIr, Operation, ParameterLocation};
use arvalez_target_core::{ClientLayout, sorted_models, sorted_operations};
use serde::Serialize;
use serde_json::{Value, from_value};
use tera::{Context as TeraContext, Tera};

use super::{TEMPLATE_CLIENT_METHOD, TEMPLATE_MODEL_INTERFACE, TEMPLATE_TAG_GROUP};
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
    pub(crate) fn from_ir(
        ir: &CoreIr,
        config: &TypeScriptPackageConfig,
        tera: &Tera,
    ) -> Result<Self> {
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
struct TsFieldView {
    name: String,
    ts_type: String,
    optional: bool,
}

#[derive(Debug, Serialize)]
struct ModelView {
    type_name: String,
    is_enum: bool,
    is_alias: bool,
    enum_expression: String,
    alias_expression: String,
    fields: Vec<TsFieldView>,
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

        let fields = model
            .fields
            .iter()
            .map(|field| TsFieldView {
                name: render_property_name(&field.name),
                ts_type: typescript_field_type(&field.type_ref, field.nullable),
                optional: field.optional,
            })
            .collect();

        Self {
            type_name: sanitize_type_name(&model.name),
            is_enum,
            is_alias,
            enum_expression,
            alias_expression: alias_type_ref
                .map(|type_ref| typescript_field_type(&type_ref, alias_nullable))
                .unwrap_or_default(),
            fields,
        }
    }
}

#[derive(Debug, Serialize)]
struct TsBindingView {
    method_name: String,
    raw_method_name: String,
}

#[derive(Debug, Serialize)]
struct TagGroupView {
    property_name: String,
    bindings: Vec<TsBindingView>,
}

impl TagGroupView {
    fn new(tag_name: String, operations: Vec<&Operation>) -> Self {
        let bindings = operations
            .into_iter()
            .map(|operation| TsBindingView {
                method_name: sanitize_identifier(&operation.name),
                raw_method_name: raw_method_name(operation),
            })
            .collect();

        Self {
            property_name: sanitize_tag_property_name(&tag_name),
            bindings,
        }
    }
}

#[derive(Debug, Serialize)]
struct TsDocParamView {
    name: String,
    description: String,
}

#[derive(Debug, Serialize)]
struct TsParamView {
    name: String,
    raw_name: String,
    required: bool,
    content_encoding: Option<String>,
}

#[derive(Debug, Serialize)]
struct TsBodyView {
    /// "json" | "form" | "binary"
    kind: String,
    required: bool,
    content_encoding: Option<String>,
}

#[derive(Debug, Serialize)]
struct OperationMethodView {
    method_name: String,
    raw_method_name: String,
    args_signature: String,
    forward_args: String,
    return_type: String,
    summary: Option<String>,
    doc_params: Vec<TsDocParamView>,
    http_method: String,
    path_template: String,
    path_params: Vec<TsParamView>,
    query_params: Vec<TsParamView>,
    header_params: Vec<TsParamView>,
    body: Option<TsBodyView>,
    response_content_encoding: Option<String>,
}

impl OperationMethodView {
    fn from_operation(operation: &Operation) -> Self {
        let summary = operation
            .attributes
            .get("summary")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned);

        let doc_params = operation
            .params
            .iter()
            .filter_map(|param| {
                param
                    .attributes
                    .get("description")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|d| !d.is_empty())
                    .map(|d| TsDocParamView {
                        name: sanitize_identifier(&param.name),
                        description: sanitize_doc_text(d),
                    })
            })
            .collect();

        let path_params = operation
            .params
            .iter()
            .filter(|p| matches!(p.location, ParameterLocation::Path))
            .map(ts_param_view)
            .collect();

        let query_params = operation
            .params
            .iter()
            .filter(|p| matches!(p.location, ParameterLocation::Query))
            .map(ts_param_view)
            .collect();

        let header_params = operation
            .params
            .iter()
            .filter(|p| matches!(p.location, ParameterLocation::Header))
            .map(ts_param_view)
            .collect();

        let body = operation.request_body.as_ref().map(|rb| {
            let kind = if rb.media_type == "application/json" {
                "json"
            } else if rb.media_type.starts_with("multipart/form-data") {
                "form"
            } else {
                "binary"
            };
            TsBodyView {
                kind: kind.into(),
                required: rb.required,
                content_encoding: content_encoding_attribute(&rb.attributes).map(ToOwned::to_owned),
            }
        });

        let response_content_encoding = operation
            .responses
            .iter()
            .find(|r| r.status.starts_with('2'))
            .and_then(|r| content_encoding_attribute(&r.attributes))
            .map(ToOwned::to_owned);

        Self {
            method_name: sanitize_identifier(&operation.name),
            raw_method_name: raw_method_name(operation),
            args_signature: build_method_args(operation).join(", "),
            forward_args: build_wrapper_forward_arguments(operation),
            return_type: operation_return_type(operation),
            summary,
            doc_params,
            http_method: http_method_string(operation.method).to_owned(),
            path_template: render_typescript_path(&operation.path),
            path_params,
            query_params,
            header_params,
            body,
            response_content_encoding,
        }
    }
}

fn ts_param_view(param: &arvalez_ir::Parameter) -> TsParamView {
    TsParamView {
        name: sanitize_identifier(&param.name),
        raw_name: param.name.clone(),
        required: param.required,
        content_encoding: content_encoding_attribute(&param.attributes).map(ToOwned::to_owned),
    }
}

fn content_encoding_attribute(attributes: &BTreeMap<String, Value>) -> Option<&str> {
    attributes.get("content_encoding").and_then(Value::as_str)
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

// ── IrEmitter helpers ───────────────────────────────────────────────────────────

/// Render a single model to a TypeScript interface declaration.
pub(crate) fn emit_model(tera: &Tera, model: &arvalez_ir::Model) -> Result<String> {
    render_model_block(tera, ModelView::from_model(model))
}

/// Render a single operation to a TypeScript client method.
pub(crate) fn emit_operation(tera: &Tera, operation: &arvalez_ir::Operation) -> Result<String> {
    render_client_method_block(tera, OperationMethodView::from_operation(operation))
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
