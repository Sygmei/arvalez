mod codegen;
mod config;
mod sanitize;
#[cfg(test)]
mod tests;
mod types;

use std::path::PathBuf;

use anyhow::{Context, Result};
use arvalez_ir::{CoreIr, Model, Operation};
pub use arvalez_target_core::GeneratedFile;
pub use arvalez_target_core::IrEmitter;
pub use arvalez_target_core::write_files as write_go_package;
use arvalez_target_core::{load_extra_package_templates, load_templates};
use tera::{Context as TeraContext, Tera};

use codegen::PackageTemplateContext;
pub use config::GoPackageConfig;

/// A Go SDK generator.
///
/// Implements [`IrEmitter`]: construct it with [`GoGenerator::new`] then call
/// [`IrEmitter::generate`] to produce the full set of output files, or call
/// [`IrEmitter::emit_model`] / [`IrEmitter::emit_operation`] for individual
/// IR elements.
pub struct GoGenerator {
    pub config: GoPackageConfig,
    pub(crate) tera: Tera,
    /// Extra user-supplied templates discovered at construction time.
    pub(crate) extra_package_templates: Vec<(String, PathBuf)>,
}

impl GoGenerator {
    /// Build a generator from `config`, compiling the Tera template engine.
    pub fn new(config: &GoPackageConfig) -> Result<Self> {
        let mut tera = load_templates(
            config.template_dir.as_deref(),
            BUILTIN_TEMPLATES,
            OVERRIDABLE_TEMPLATES,
        )?;
        let extra_package_templates = if let Some(dir) = config.template_dir.as_deref() {
            load_extra_package_templates(dir, OVERRIDABLE_TEMPLATES, &mut tera)?
        } else {
            Vec::new()
        };
        Ok(Self {
            config: config.clone(),
            tera,
            extra_package_templates,
        })
    }
}

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

/// Generate a Go client package from an IR snapshot.
///
/// Convenience wrapper around [`GoGenerator::new`] + [`IrEmitter::generate`].
pub fn generate_go_package(ir: &CoreIr, config: &GoPackageConfig) -> Result<Vec<GeneratedFile>> {
    GoGenerator::new(config)?.generate(ir)
}

impl IrEmitter for GoGenerator {
    fn emit_model(&self, _ir: &CoreIr, model: &Model) -> Result<String> {
        codegen::emit_model(&self.tera, model)
    }

    fn emit_operation(&self, _ir: &CoreIr, operation: &Operation) -> Result<String> {
        codegen::emit_operation(&self.tera, operation)
    }

    fn generate(&self, ir: &CoreIr) -> Result<Vec<GeneratedFile>> {
        assemble_go_files(&self.tera, &self.config, ir, &self.extra_package_templates)
    }
}

pub(crate) fn assemble_go_files(
    tera: &Tera,
    config: &GoPackageConfig,
    ir: &CoreIr,
    extra_package_templates: &[(String, PathBuf)],
) -> Result<Vec<GeneratedFile>> {
    let package_context = PackageTemplateContext::from_ir(ir, config, tera)?;
    let mut context = TeraContext::new();
    context.insert("package", &package_context);
    let mut files = vec![
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
    ];

    for (template_name, output_path) in extra_package_templates {
        let contents = tera
            .render(template_name, &context)
            .with_context(|| format!("failed to render extra template `{template_name}`"))?;
        files.push(GeneratedFile {
            path: output_path.clone(),
            contents,
        });
    }

    Ok(files)
}
