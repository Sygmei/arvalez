use std::path::PathBuf;

use anyhow::{Context, Result};
use arvalez_ir::{CoreIr, Model, Operation};
pub use arvalez_target_core::GeneratedFile;
pub use arvalez_target_core::IrEmitter;
pub use arvalez_target_core::write_files as write_nushell_package;
use arvalez_target_core::{collect_erased_templates, load_extra_package_templates, load_templates};
use tera::{Context as TeraContext, Tera};

mod codegen;
mod config;
mod sanitize;
mod types;
#[cfg(test)]
mod tests;

pub use config::NushellPackageConfig;
use codegen::PackageTemplateContext;

/// A Nushell SDK generator.
///
/// Implements [`IrEmitter`]: construct it with [`NushellGenerator::new`] then
/// call [`IrEmitter::generate`] to produce the full set of output files, or
/// call [`IrEmitter::emit_model`] / [`IrEmitter::emit_operation`] to render
/// individual IR elements in isolation.
pub struct NushellGenerator {
    pub config: NushellPackageConfig,
    pub(crate) tera: Tera,
    /// Extra user-supplied templates discovered at construction time.
    pub(crate) extra_package_templates: Vec<(String, PathBuf)>,
    /// Default templates suppressed by a tilde-prefixed eraser file.
    pub(crate) erased_templates: Vec<String>,
}

impl NushellGenerator {
    /// Build a generator from `config`, compiling the Tera template engine.
    pub fn new(config: &NushellPackageConfig) -> Result<Self> {
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
        let erased_templates = if let Some(dir) = config.template_dir.as_deref() {
            collect_erased_templates(dir, OVERRIDABLE_TEMPLATES)
        } else {
            Vec::new()
        };
        Ok(Self { config: config.clone(), tera, extra_package_templates, erased_templates })
    }
}

const TEMPLATE_README: &str = "package/README.md.tera";
const TEMPLATE_MOD: &str = "package/mod.nu.tera";
const TEMPLATE_CLIENT: &str = "package/client.nu.tera";
const TEMPLATE_MODELS: &str = "package/models.nu.tera";
const TEMPLATE_COMMAND: &str = "partials/command.nu.tera";
const TEMPLATE_MODEL_RECORD: &str = "partials/model_record.nu.tera";

const BUILTIN_TEMPLATES: &[(&str, &str)] = &[
    (TEMPLATE_README, include_str!("../templates/package/README.md.tera")),
    (TEMPLATE_MOD, include_str!("../templates/package/mod.nu.tera")),
    (TEMPLATE_CLIENT, include_str!("../templates/package/client.nu.tera")),
    (TEMPLATE_MODELS, include_str!("../templates/package/models.nu.tera")),
    (TEMPLATE_COMMAND, include_str!("../templates/partials/command.nu.tera")),
    (TEMPLATE_MODEL_RECORD, include_str!("../templates/partials/model_record.nu.tera")),
];

const OVERRIDABLE_TEMPLATES: &[&str] = &[
    TEMPLATE_README,
    TEMPLATE_MOD,
    TEMPLATE_CLIENT,
    TEMPLATE_MODELS,
    TEMPLATE_COMMAND,
    TEMPLATE_MODEL_RECORD,
];

/// Generate a Nushell client module from an IR snapshot.
///
/// Convenience wrapper around [`NushellGenerator::new`] + [`IrEmitter::generate`].
pub fn generate_nushell_package(
    ir: &CoreIr,
    config: &NushellPackageConfig,
) -> Result<Vec<GeneratedFile>> {
    NushellGenerator::new(config)?.generate(ir)
}

impl IrEmitter for NushellGenerator {
    fn emit_model(&self, ir: &CoreIr, model: &Model) -> Result<String> {
        codegen::emit_model(&self.tera, ir, model)
    }

    fn emit_operation(&self, ir: &CoreIr, operation: &Operation) -> Result<String> {
        codegen::emit_operation(&self.tera, ir, operation, &self.config)
    }

    fn generate(&self, ir: &CoreIr) -> Result<Vec<GeneratedFile>> {
        assemble_nushell_files(
            &self.tera,
            &self.config,
            ir,
            &self.extra_package_templates,
            &self.erased_templates,
        )
    }
}

// ── Internal assembly used by IrEmitter::generate ────────────────────────────

pub(crate) fn assemble_nushell_files(
    tera: &Tera,
    config: &NushellPackageConfig,
    ir: &CoreIr,
    extra_package_templates: &[(String, PathBuf)],
    erased_templates: &[String],
) -> Result<Vec<GeneratedFile>> {
    let package_context = PackageTemplateContext::from_ir(ir, config, tera)?;
    let mut ctx = TeraContext::new();
    ctx.insert("package", &package_context);

    let is_erased = |name: &str| erased_templates.iter().any(|e| e == name);

    let mut files = Vec::new();

    if !is_erased(TEMPLATE_README) {
        files.push(GeneratedFile {
            path: PathBuf::from("README.md"),
            contents: tera
                .render(TEMPLATE_README, &ctx)
                .context("failed to render README template")?,
        });
    }
    if !is_erased(TEMPLATE_MOD) {
        files.push(GeneratedFile {
            path: PathBuf::from("mod.nu"),
            contents: tera
                .render(TEMPLATE_MOD, &ctx)
                .context("failed to render mod.nu template")?,
        });
    }
    if !is_erased(TEMPLATE_CLIENT) {
        files.push(GeneratedFile {
            path: PathBuf::from("client.nu"),
            contents: tera
                .render(TEMPLATE_CLIENT, &ctx)
                .context("failed to render client.nu template")?,
        });
    }
    if !is_erased(TEMPLATE_MODELS) {
        files.push(GeneratedFile {
            path: PathBuf::from("models.nu"),
            contents: tera
                .render(TEMPLATE_MODELS, &ctx)
                .context("failed to render models.nu template")?,
        });
    }

    for (template_name, output_path) in extra_package_templates {
        let contents = tera
            .render(template_name, &ctx)
            .with_context(|| format!("failed to render extra template `{template_name}`"))?;
        files.push(GeneratedFile { path: output_path.clone(), contents });
    }

    Ok(files)
}
