//! Terraform models and typed adapters for the standard Arvalez Go SDK.
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{Context, Result, bail};
use arvalez_ir::{CoreIr, Field, Model, ModelKind, TypeRef};
use arvalez_target_core::{CommonConfig, GeneratedFile};
use arvalez_target_go::{model_field_is_pointer, model_identifier, model_value_type};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub use arvalez_target_core::write_files as write_package;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetConfig {
    pub module_path: String,
    pub sdk_module_path: String,
    /// "current" resolves to CommonConfig.package.version for direct library callers.
    #[serde(default = "current_sdk_version")]
    pub sdk_version: String,
    #[serde(default)]
    pub sdk_import_path: Option<String>,
    #[serde(default)]
    pub models: BTreeMap<String, ModelOptions>,
}

fn current_sdk_version() -> String {
    "current".into()
}

impl Default for TargetConfig {
    fn default() -> Self {
        Self {
            module_path: String::new(),
            sdk_module_path: String::new(),
            sdk_version: current_sdk_version(),
            sdk_import_path: None,
            models: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelOptions {
    #[serde(default)]
    pub fields: BTreeMap<String, FieldOptions>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FieldOptions {
    pub name: Option<String>,
    pub representation: Option<Representation>,
    #[serde(default)]
    pub null_behavior: NullBehavior,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Representation {
    Json,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NullBehavior {
    #[default]
    Omit,
    ExplicitNull,
}

pub const TEMPLATES: &[(&str, &str)] = &[
    (
        "root/go.mod.tera",
        include_str!("../templates/root/go.mod.tera"),
    ),
    (
        "root/README.md.tera",
        include_str!("../templates/root/README.md.tera"),
    ),
    (
        "root/models.go.tera",
        include_str!("../templates/root/models.go.tera"),
    ),
    (
        "root/adapters.go.tera",
        include_str!("../templates/root/adapters.go.tera"),
    ),
    (
        "root/schema.go.tera",
        include_str!("../templates/root/schema.go.tera"),
    ),
    (
        "root/runtime.go.tera",
        include_str!("../templates/root/runtime.go.tera"),
    ),
    (
        "partials/model.go.tera",
        include_str!("../templates/partials/model.go.tera"),
    ),
    (
        "partials/adapter.go.tera",
        include_str!("../templates/partials/adapter.go.tera"),
    ),
];

// Reuse the common template dump/eraser implementation; generation below adds a
// validated Terraform-specific view before invoking the same root renderer.
mod template_tools {
    use super::*;
    #[allow(dead_code)]
    fn filters(_: &mut tera::Tera) {}
    arvalez_target_core::declare_target! { config: TargetConfig, templates: TEMPLATES, filters: filters }
}
pub use template_tools::{dump_erasers, dump_templates};

fn valid_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c == '_')
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}
fn valid_module(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"/._-~".contains(&c))
        && value
            .split('/')
            .all(|s| !s.is_empty() && s != "." && s != "..")
}
fn valid_version(value: &str) -> bool {
    let Some(version) = value.strip_prefix('v') else {
        return false;
    };
    let core = version.split(['-', '+']).next().unwrap_or("");
    let parts: Vec<_> = core.split('.').collect();
    parts.len() == 3
        && parts.iter().all(|p| {
            !p.is_empty()
                && p.bytes().all(|b| b.is_ascii_digit())
                && (p.len() == 1 || !p.starts_with('0'))
        })
        && version
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".+-".contains(&b))
        && !version.ends_with(['-', '+', '.'])
}

fn attribute_name(field: &Field, options: &FieldOptions) -> Result<String> {
    let name = options.name.clone().unwrap_or_else(|| {
        let name = arvalez_target_core::to_snake_case(&field.name);
        if matches!(
            name.as_str(),
            "count"
                | "for_each"
                | "depends_on"
                | "provider"
                | "lifecycle"
                | "provisioner"
                | "connection"
        ) {
            format!("{name}_value")
        } else {
            name
        }
    });
    if !valid_identifier(&name) {
        bail!("invalid Terraform attribute name {name:?}; configure a field name override");
    }
    if matches!(
        name.as_str(),
        "count"
            | "for_each"
            | "depends_on"
            | "provider"
            | "lifecycle"
            | "provisioner"
            | "connection"
    ) {
        bail!("reserved Terraform attribute name {name:?}");
    }
    Ok(name)
}

struct Shape {
    tf: &'static str,
    codec: String,
}
struct Resolver<'a> {
    ir: &'a CoreIr,
    config: &'a TargetConfig,
    json_fields: BTreeMap<(String, String), &'static str>,
}
// A collection containing an unsupported value uses JSON for the whole field.
// Object contents are handled by their own model, preserving typed parent fields.
fn supports_native_type(ty: &TypeRef) -> bool {
    match ty {
        TypeRef::Primitive { name } => matches!(
            name.as_str(),
            "string" | "integer" | "number" | "boolean" | "binary"
        ),
        TypeRef::Named { .. } => true,
        TypeRef::Enum { base, .. } => supports_native_type(base),
        TypeRef::Array { item } => supports_native_type(item),
        TypeRef::Map { value } => supports_native_type(value),
        _ => false,
    }
}

fn model_references(ty: &TypeRef, names: &mut BTreeSet<String>) {
    match ty {
        TypeRef::Named { name } => {
            names.insert(name.clone());
        }
        TypeRef::Enum { base, .. } => model_references(base, names),
        TypeRef::Array { item } => model_references(item, names),
        TypeRef::Map { value } => model_references(value, names),
        _ => {}
    }
}

fn reaches_model(
    graph: &BTreeMap<String, BTreeSet<String>>,
    start: &str,
    target: &str,
    visited: &mut BTreeSet<String>,
) -> bool {
    if start == target {
        return true;
    }
    if !visited.insert(start.into()) {
        return false;
    }
    graph.get(start).is_some_and(|names| {
        names
            .iter()
            .any(|name| reaches_model(graph, name, target, visited))
    })
}

impl<'a> Resolver<'a> {
    fn new(ir: &'a CoreIr, config: &'a TargetConfig) -> Self {
        let mut resolver = Self {
            ir,
            config,
            json_fields: BTreeMap::new(),
        };
        let mut graph = BTreeMap::<String, BTreeSet<String>>::new();
        for model in &ir.models {
            let references = graph.entry(model.name.clone()).or_default();
            for field in &model.fields {
                let reason = if resolver.options(model, field).representation
                    == Some(Representation::Json)
                {
                    Some("explicit override")
                } else if !supports_native_type(&field.type_ref) {
                    Some("type without a native Terraform representation")
                } else {
                    None
                };
                if let Some(reason) = reason {
                    resolver
                        .json_fields
                        .insert((model.name.clone(), field.name.clone()), reason);
                } else {
                    model_references(&field.type_ref, references);
                }
            }
            if let ModelKind::Enum { base, .. } = &model.kind {
                model_references(base, references);
            }
        }
        // Cut every field edge participating in a cycle. Compute against the
        // same graph for every model so traversal order cannot change schemas.
        for model in &ir.models {
            for field in &model.fields {
                let key = (model.name.clone(), field.name.clone());
                if resolver.json_fields.contains_key(&key) {
                    continue;
                }
                let mut references = BTreeSet::new();
                model_references(&field.type_ref, &mut references);
                if references
                    .iter()
                    .any(|name| reaches_model(&graph, name, &model.name, &mut BTreeSet::new()))
                {
                    resolver
                        .json_fields
                        .insert(key, "recursive model reference");
                }
            }
        }
        resolver
    }
    fn options(&self, model: &Model, field: &Field) -> FieldOptions {
        self.config
            .models
            .get(&model.name)
            .and_then(|m| m.fields.get(&field.name))
            .cloned()
            .unwrap_or_default()
    }
    fn field(&self, model: &Model, field: &Field, stack: &mut Vec<String>) -> Result<Shape> {
        let options = self.options(model, field);
        if options.null_behavior == NullBehavior::ExplicitNull
            && !(field.optional && field.nullable)
        {
            bail!("explicit_null requires an optional nullable field");
        }
        let mut shape = if self
            .json_fields
            .contains_key(&(model.name.clone(), field.name.clone()))
        {
            Shape {
                tf: "types.String",
                codec: format!("jsonCodec[{}]()", model_value_type(&field.type_ref, "sdk")),
            }
        } else {
            self.shape(&field.type_ref, stack)?
        };
        if field.nullable {
            shape.codec = format!("nullableCodec({})", shape.codec);
            if field.optional {
                shape.codec = format!(
                    "optionalNullableCodec({}, {})",
                    shape.codec,
                    options.null_behavior == NullBehavior::ExplicitNull
                );
            }
        } else if model_field_is_pointer(field) {
            shape.codec = format!("pointerCodec({})", shape.codec);
        } else if field.optional
            && !matches!(
                &field.type_ref,
                TypeRef::Tuple {
                    rest: arvalez_ir::TupleRest::Forbidden,
                    ..
                }
            )
        {
            shape.codec = format!("optionalValueCodec({})", shape.codec);
        } else {
            shape.codec = format!("nonNullCodec({})", shape.codec);
        }
        Ok(shape)
    }
    fn shape(&self, ty: &TypeRef, stack: &mut Vec<String>) -> Result<Shape> {
        let (tf, codec) = match ty {
            TypeRef::Primitive { name } => match name.as_str() {
                "string" => ("types.String", "stringCodec()".into()),
                "integer" => ("types.Int64", "int64Codec()".into()),
                "number" => ("types.Float64", "float64Codec()".into()),
                "boolean" => ("types.Bool", "boolCodec()".into()),
                "binary" => ("types.String", "binaryCodec()".into()),
                _ => bail!("unsupported primitive {name:?} outside an object field"),
            },
            TypeRef::Enum { base, .. } => return self.shape(base, stack),
            TypeRef::Named { name } => {
                if stack.contains(name) {
                    bail!("recursive model {name:?} cannot be expanded outside an object field");
                }
                let model = self
                    .ir
                    .models
                    .iter()
                    .find(|m| &m.name == name)
                    .with_context(|| format!("unknown SDK model {name:?}"))?;
                stack.push(name.clone());
                let tf = match &model.kind {
                    ModelKind::Object => {
                        for field in &model.fields {
                            self.field(model, field, stack)
                                .with_context(|| format!("{}.{}", model.name, field.name))?;
                        }
                        "types.Object"
                    }
                    ModelKind::Enum { base, .. } => {
                        let shape = self.shape(base, stack)?;
                        if !matches!(
                            shape.tf,
                            "types.String" | "types.Int64" | "types.Float64" | "types.Bool"
                        ) {
                            bail!("named enum {name:?} must have a scalar base");
                        }
                        shape.tf
                    }
                };
                stack.pop();
                (tf, format!("codec{}()", model_identifier(name)))
            }
            TypeRef::Array { item } => {
                let child = self.shape(item, stack)?;
                ("types.List", format!("listCodec({})", child.codec))
            }
            TypeRef::Map { value } => {
                let child = self.shape(value, stack)?;
                ("types.Map", format!("mapCodec({})", child.codec))
            }
            _ => bail!("unsupported union or tuple outside an object field"),
        };
        Ok(Shape { tf, codec })
    }
    fn model(&self, model: &Model) -> Result<Value> {
        let name = model_identifier(&model.name);
        let shape = self.shape(&TypeRef::named(&model.name), &mut Vec::new())?;
        let mut fields = Vec::new();
        let mut attrs = BTreeSet::new();
        let mut go_names = BTreeSet::new();
        for field in &model.fields {
            let options = self.options(model, field);
            let attr = attribute_name(field, &options)?;
            if !attrs.insert(attr.clone()) {
                bail!(
                    "model {} has colliding Terraform attribute {attr:?}; configure distinct field names",
                    model.name
                );
            }
            let go = model_identifier(&field.name);
            if !go_names.insert(go.clone()) {
                bail!("model {} has colliding SDK field {go:?}", model.name);
            }
            let shape = self
                .field(model, field, &mut vec![model.name.clone()])
                .with_context(|| format!("{}.{}", model.name, field.name))?;
            fields.push(json!({ "name": go, "attribute": attr, "tf_type": shape.tf, "codec": shape.codec,
                "json_reason": self.json_fields.get(&(model.name.clone(), field.name.clone())),
                "description": serde_json::to_string(field.attributes.get("description").and_then(Value::as_str).unwrap_or(""))? }));
        }
        let (base, base_codec) = match &model.kind {
            ModelKind::Enum { base, .. } => (
                model_value_type(base, "sdk"),
                self.shape(base, &mut vec![model.name.clone()])?.codec,
            ),
            _ => (String::new(), String::new()),
        };
        Ok(
            json!({ "name": name, "fields": fields, "object": matches!(model.kind, ModelKind::Object),
            "tf_type": shape.tf, "base": base, "base_codec": base_codec }),
        )
    }
}

/// Generate without invoking Go or downloading dependencies. All validation is
/// performed before returning files, so callers can avoid partial output.
pub fn generate(
    ir: &CoreIr,
    template_dir: Option<&Path>,
    common: &CommonConfig,
    config: &TargetConfig,
) -> Result<Vec<GeneratedFile>> {
    let mut resolved = std::borrow::Cow::Borrowed(config);
    if config.sdk_version == "current" {
        let version = &common.package.version;
        resolved.to_mut().sdk_version = if version.starts_with('v') {
            version.clone()
        } else {
            format!("v{version}")
        };
    }
    let config = resolved.as_ref();
    for (key, value) in [
        ("module_path", &config.module_path),
        ("sdk_module_path", &config.sdk_module_path),
    ] {
        if !valid_module(value) {
            bail!("terraform {key} is required and must be a valid Go module path");
        }
    }
    let import = config
        .sdk_import_path
        .as_deref()
        .unwrap_or(&config.sdk_module_path);
    if !valid_module(import)
        || !(import == config.sdk_module_path
            || import.starts_with(&format!("{}/", config.sdk_module_path)))
    {
        bail!("sdk_import_path must be inside sdk_module_path");
    }
    if config.module_path == config.sdk_module_path {
        bail!("Terraform and SDK modules must have distinct paths");
    }
    if !valid_version(&config.sdk_version) {
        bail!(
            "sdk_version must resolve to a pinned Go semantic version (for example v2.0.0); current uses the SDK package version"
        );
    }

    if !valid_identifier(&common.package.name)
        || [
            "type",
            "map",
            "func",
            "var",
            "package",
            "import",
            "range",
            "default",
            "interface",
            "struct",
            "chan",
            "const",
            "go",
            "select",
            "defer",
            "return",
            "if",
            "else",
            "for",
            "switch",
            "case",
            "break",
            "continue",
            "fallthrough",
            "goto",
            "_",
        ]
        .contains(&common.package.name.as_str())
    {
        bail!(
            "invalid Terraform Go package name {:?}",
            common.package.name
        );
    }
    for (name, options) in &config.models {
        let model = ir
            .models
            .iter()
            .find(|m| &m.name == name)
            .with_context(|| format!("unknown Terraform model override {name:?}"))?;
        for field in options.fields.keys() {
            if !model.fields.iter().any(|f| &f.name == field) {
                bail!("unknown Terraform field override {name}.{field}");
            }
        }
    }
    let resolver = Resolver::new(ir, config);
    let mut names = BTreeSet::new();
    let mut models = Vec::new();
    for model in arvalez_target_core::sorted_models(ir) {
        if !names.insert(model_identifier(&model.name)) {
            bail!("colliding SDK model name {:?}", model.name);
        }
        models.push(resolver.model(model)?);
    }
    let tera = arvalez_target_core::load_templates(template_dir, TEMPLATES)?;
    let erased = template_dir
        .map(|d| arvalez_target_core::collect_erased_root_templates(d, TEMPLATES))
        .unwrap_or_default();
    let mut context = tera::Context::new();
    context.insert("tf_models", &models);
    context.insert("sdk_import", import);
    arvalez_target_core::render_root_templates(
        &tera,
        ir,
        context,
        TEMPLATES,
        common,
        &serde_json::to_value(config)?,
        &erased,
    )
}

#[cfg(test)]
mod tests;
