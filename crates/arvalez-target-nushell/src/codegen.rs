use anyhow::{Context, Result};
use arvalez_ir::{CoreIr, Operation, ParameterLocation};
use arvalez_target_core::{operation_primary_tag, sorted_models, sorted_operations};
use serde::Serialize;
use tera::{Context as TeraContext, Tera};

use super::{TEMPLATE_COMMAND, TEMPLATE_MODEL_RECORD};
use crate::config::NushellPackageConfig;
use crate::sanitize::{
    sanitize_command_name, sanitize_field_name, sanitize_flag_name, sanitize_variable_name,
};
use crate::types::{TypeRegistry, build_type_registry, http_verb, nushell_type_ref, render_nu_path};

#[derive(Debug, Serialize)]
pub(crate) struct PackageTemplateContext {
    pub module_name: String,
    pub version: String,
    pub default_base_url: String,
    pub command_blocks: Vec<String>,
    pub model_blocks: Vec<String>,
}

impl PackageTemplateContext {
    pub(crate) fn from_ir(ir: &CoreIr, config: &NushellPackageConfig, tera: &Tera) -> Result<Self> {
        let registry = build_type_registry(ir);
        let command_blocks = sorted_operations(ir)
            .into_iter()
            .map(|op| render_command_block(tera, OperationCommandView::from_operation(op, config, &registry)))
            .collect::<Result<Vec<_>>>()?;
        let model_blocks = sorted_models(ir)
            .into_iter()
            .map(|model| render_model_record_block(tera, ModelRecordView::from_model(model, &registry)))
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            module_name: config.module_name.clone(),
            version: config.version.clone(),
            default_base_url: config.default_base_url.clone(),
            command_blocks,
            model_blocks,
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ParamView {
    /// Sanitised variable name used in the command body (`$name`).
    pub name: String,
    /// Original parameter name used as URL key or header key.
    pub raw_name: String,
    /// Kebab-case name used as `--flag-name` in the command signature.
    pub flag_name: String,
    pub nu_type: String,
    pub description: String,
    pub required: bool,
}

#[derive(Debug, Serialize)]
pub(crate) struct OperationCommandView {
    /// Full command name, including tag prefix when `group_by_tag` is enabled.
    pub command_name: String,
    /// Sanitised primary tag (empty string when the operation is untagged).
    pub tag: String,
    pub summary: String,
    pub http_verb: String,
    /// Path parameters — rendered as positional args in the command signature.
    pub positional_params: Vec<ParamView>,
    /// Query + header parameters combined — rendered as `--flags` in the signature.
    pub flag_params: Vec<ParamView>,
    /// Query parameters only — used in the template to build the request URL.
    pub query_params: Vec<ParamView>,
    /// Header parameters only — used in the template to build the headers record.
    pub header_params: Vec<ParamView>,
    pub has_body: bool,
    pub body_required: bool,
    pub body_nu_type: String,
    pub content_type: String,
    pub return_nu_type: String,
    pub path_template: String,
    pub default_base_url: String,
}

impl OperationCommandView {
    fn from_operation(op: &Operation, config: &NushellPackageConfig, registry: &TypeRegistry) -> Self {
        let summary = op.attributes.get("summary").and_then(|v| v.as_str()).unwrap_or("").trim().to_owned();
        let positional_params: Vec<ParamView> = op.params.iter()
            .filter(|p| matches!(p.location, ParameterLocation::Path))
            .map(|p| ParamView {
                name: sanitize_variable_name(&p.name),
                raw_name: p.name.clone(),
                flag_name: sanitize_flag_name(&p.name),
                nu_type: nushell_type_ref(&p.type_ref, registry),
                description: p.attributes.get("description").and_then(|v| v.as_str()).unwrap_or("").trim().to_owned(),
                required: p.required,
            })
            .collect();
        let query_params: Vec<ParamView> = op.params.iter()
            .filter(|p| matches!(p.location, ParameterLocation::Query))
            .map(|p| ParamView {
                name: sanitize_variable_name(&p.name),
                raw_name: p.name.clone(),
                flag_name: sanitize_flag_name(&p.name),
                nu_type: nushell_type_ref(&p.type_ref, registry),
                description: p.attributes.get("description").and_then(|v| v.as_str()).unwrap_or("").trim().to_owned(),
                required: p.required,
            })
            .collect();
        let header_params: Vec<ParamView> = op.params.iter()
            .filter(|p| matches!(p.location, ParameterLocation::Header))
            .map(|p| ParamView {
                name: sanitize_variable_name(&p.name),
                raw_name: p.name.clone(),
                flag_name: sanitize_flag_name(&p.name),
                nu_type: nushell_type_ref(&p.type_ref, registry),
                description: p.attributes.get("description").and_then(|v| v.as_str()).unwrap_or("").trim().to_owned(),
                required: p.required,
            })
            .collect();
        let flag_params: Vec<ParamView> = query_params.iter().chain(header_params.iter()).cloned().collect();
        let has_body = op.request_body.is_some();
        let (body_required, body_nu_type, content_type) = op.request_body.as_ref()
            .map(|rb| {
                let nu_type = rb.type_ref.as_ref().map(|t| nushell_type_ref(t, registry)).unwrap_or_else(|| "any".into());
                (rb.required, nu_type, rb.media_type.clone())
            })
            .unwrap_or_else(|| (false, "any".into(), String::new()));
        let return_nu_type = op.responses.iter()
            .find(|r| r.status.starts_with('2'))
            .and_then(|r| r.type_ref.as_ref())
            .map(|t| nushell_type_ref(t, registry))
            .unwrap_or_else(|| "any".into());
        let primary_tag = operation_primary_tag(op);
        let tag = primary_tag.as_deref().map(sanitize_command_name).unwrap_or_default();
        let base_name = sanitize_command_name(&op.name);
        let command_name = if config.group_by_tag && !tag.is_empty() {
            format!("{} {}", tag, base_name)
        } else {
            base_name
        };
        Self {
            command_name,
            tag,
            summary,
            http_verb: http_verb(op.method).to_owned(),
            positional_params,
            flag_params,
            query_params,
            header_params,
            has_body,
            body_required,
            body_nu_type,
            content_type,
            return_nu_type,
            path_template: render_nu_path(&op.path),
            default_base_url: config.default_base_url.clone(),
        }
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct FieldView {
    pub field_name: String,
    pub nu_type: String,
    pub optional: bool,
    pub description: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct EnumVariantView {
    pub raw: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct ModelRecordView {
    pub type_name: String,
    pub command_name: String,
    /// Nushell typed record annotation, e.g. `record<id: string, count: int>`.
    pub typed_record_type: String,
    pub is_enum: bool,
    pub enum_variants: Vec<EnumVariantView>,
    pub fields: Vec<FieldView>,
}

impl ModelRecordView {
    fn from_model(model: &arvalez_ir::Model, registry: &TypeRegistry) -> Self {
        let is_enum = model.attributes.contains_key("enum_values");
        let enum_variants = if is_enum {
            model.attributes.get("enum_values").and_then(|v| v.as_array())
                .map(|vals| vals.iter().map(|v| EnumVariantView {
                    raw: match v {
                        serde_json::Value::String(s) => format!("{s:?}"),
                        other => other.to_string(),
                    },
                }).collect())
                .unwrap_or_default()
        } else { Vec::new() };
        let fields = model.fields.iter().map(|f| FieldView {
            field_name: sanitize_field_name(&f.name),
            nu_type: nushell_type_ref(&f.type_ref, registry),
            optional: f.optional,
            description: f.attributes.get("description").and_then(|v| v.as_str()).unwrap_or("").trim().to_owned(),
        }).collect();
        let typed_record_type = if is_enum {
            "string".to_owned()
        } else {
            registry.get(&model.name).cloned().unwrap_or_else(|| "record".into())
        };
        Self {
            type_name: sanitize_command_name(&model.name),
            command_name: format!("make-{}", sanitize_command_name(&model.name)),
            typed_record_type,
            is_enum,
            enum_variants,
            fields,
        }
    }
}

fn render_command_block(tera: &Tera, view: OperationCommandView) -> Result<String> {
    let mut ctx = TeraContext::new();
    ctx.insert("operation", &view);
    tera.render(TEMPLATE_COMMAND, &ctx).context("failed to render command partial")
}

fn render_model_record_block(tera: &Tera, view: ModelRecordView) -> Result<String> {
    let mut ctx = TeraContext::new();
    ctx.insert("model", &view);
    tera.render(TEMPLATE_MODEL_RECORD, &ctx).context("failed to render model record partial")
}

// ── IrEmitter helpers ───────────────────────────────────────────────────────────

/// Render a single model to a Nushell `make-<name>` command block.
pub(crate) fn emit_model(
    tera: &Tera,
    ir: &CoreIr,
    model: &arvalez_ir::Model,
) -> Result<String> {
    let registry = build_type_registry(ir);
    render_model_record_block(tera, ModelRecordView::from_model(model, &registry))
}

/// Render a single operation to a Nushell command block.
pub(crate) fn emit_operation(
    tera: &Tera,
    ir: &CoreIr,
    operation: &arvalez_ir::Operation,
    config: &NushellPackageConfig,
) -> Result<String> {
    let registry = build_type_registry(ir);
    render_command_block(tera, OperationCommandView::from_operation(operation, config, &registry))
}
