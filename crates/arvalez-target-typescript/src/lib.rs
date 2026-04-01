use std::path::PathBuf;

use anyhow::{Context, Result};
use arvalez_ir::{CoreIr, Model, Operation};
pub use arvalez_target_core::GeneratedFile;
pub use arvalez_target_core::write_files as write_typescript_package;
use arvalez_target_core::{collect_erased_templates, load_extra_package_templates, load_templates};
pub use arvalez_target_core::IrEmitter;
use tera::{Context as TeraContext, Tera};

mod config;
mod sanitize;
mod types;
mod codegen;
#[cfg(test)]
mod tests;

pub use config::TypeScriptPackageConfig;
use codegen::PackageTemplateContext;

/// A TypeScript SDK generator.
///
/// Implements [`IrEmitter`]: construct it with [`TypeScriptGenerator::new`] then
/// call [`IrEmitter::generate`] or the individual `emit_*` methods.
pub struct TypeScriptGenerator {
    pub config: TypeScriptPackageConfig,
    pub(crate) tera: Tera,
    /// Extra user-supplied templates discovered at construction time.
    pub(crate) extra_package_templates: Vec<(String, PathBuf)>,
    /// Default templates suppressed by a tilde-prefixed eraser file.
    pub(crate) erased_templates: Vec<String>,
}

impl TypeScriptGenerator {
    /// Build a generator from `config`, compiling the Tera template engine.
    pub fn new(config: &TypeScriptPackageConfig) -> Result<Self> {
        let mut tera = load_templates(
            config.template_dir.as_deref(),
            BUILTIN_TEMPLATES,
        )?;
        let extra_package_templates = if let Some(dir) = config.template_dir.as_deref() {
            load_extra_package_templates(dir, OVERRIDABLE_TEMPLATES, &mut tera)?
        } else {
            Vec::new()
        };
        let erased_templates = if let Some(dir) = config.template_dir.as_deref() {
            collect_erased_templates(dir, OVERRIDABLE_TEMPLATES)
        } else {
            Vec::new()
        };
        Ok(Self { config: config.clone(), tera, extra_package_templates, erased_templates })
    }
}

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

/// Generate a TypeScript client package from an IR snapshot.
///
/// Convenience wrapper around [`TypeScriptGenerator::new`] + [`IrEmitter::generate`].
pub fn generate_typescript_package(
    ir: &CoreIr,
    config: &TypeScriptPackageConfig,
) -> Result<Vec<GeneratedFile>> {
    TypeScriptGenerator::new(config)?.generate(ir)
}

impl IrEmitter for TypeScriptGenerator {
    fn emit_model(&self, _ir: &CoreIr, model: &Model) -> Result<String> {
        codegen::emit_model(&self.tera, model)
    }

    fn emit_operation(&self, _ir: &CoreIr, operation: &Operation) -> Result<String> {
        codegen::emit_operation(&self.tera, operation)
    }

    fn generate(&self, ir: &CoreIr) -> Result<Vec<GeneratedFile>> {
        assemble_typescript_files(
            &self.tera,
            &self.config,
            ir,
            &self.extra_package_templates,
            &self.erased_templates,
        )
    }
}

pub(crate) fn assemble_typescript_files(
    tera: &Tera,
    config: &TypeScriptPackageConfig,
    ir: &CoreIr,
    extra_package_templates: &[(String, PathBuf)],
    erased_templates: &[String],
) -> Result<Vec<GeneratedFile>> {
    let package_context = PackageTemplateContext::from_ir(ir, config, tera)?;
    let mut template_context = TeraContext::new();
    template_context.insert("package", &package_context);

    let is_erased = |name: &str| erased_templates.iter().any(|e| e == name);

    let mut files = Vec::new();

    if !is_erased(TEMPLATE_PACKAGE_JSON) {
        files.push(GeneratedFile {
            path: PathBuf::from("package.json"),
            contents: tera
                .render(TEMPLATE_PACKAGE_JSON, &template_context)
                .context("failed to render package.json template")?,
        });
    }
    if !is_erased(TEMPLATE_TSCONFIG) {
        files.push(GeneratedFile {
            path: PathBuf::from("tsconfig.json"),
            contents: tera
                .render(TEMPLATE_TSCONFIG, &template_context)
                .context("failed to render tsconfig template")?,
        });
    }
    if !is_erased(TEMPLATE_README) {
        files.push(GeneratedFile {
            path: PathBuf::from("README.md"),
            contents: tera
                .render(TEMPLATE_README, &template_context)
                .context("failed to render README template")?,
        });
    }
    if !is_erased(TEMPLATE_MODELS) {
        files.push(GeneratedFile {
            path: PathBuf::from("src").join("models.ts"),
            contents: tera
                .render(TEMPLATE_MODELS, &template_context)
                .context("failed to render models template")?,
        });
    }
    if !is_erased(TEMPLATE_CLIENT) {
        files.push(GeneratedFile {
            path: PathBuf::from("src").join("client.ts"),
            contents: tera
                .render(TEMPLATE_CLIENT, &template_context)
                .context("failed to render client template")?,
        });
    }
    if !is_erased(TEMPLATE_INDEX) {
        files.push(GeneratedFile {
            path: PathBuf::from("src").join("index.ts"),
            contents: tera
                .render(TEMPLATE_INDEX, &template_context)
                .context("failed to render index template")?,
        });
    }

    for (template_name, output_path) in extra_package_templates {
        let contents = tera
            .render(template_name, &template_context)
            .with_context(|| format!("failed to render extra template `{template_name}`"))?;
        files.push(GeneratedFile { path: output_path.clone(), contents });
    }

    Ok(files)
}
