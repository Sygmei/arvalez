use std::path::PathBuf;

use anyhow::{Context, Result};
use arvalez_ir::{CoreIr, Model, Operation};
pub use arvalez_target_core::GeneratedFile;
pub use arvalez_target_core::IrEmitter;
use arvalez_target_core::{load_extra_package_templates, load_templates};
pub use arvalez_target_core::write_files as write_python_package;
use tera::{Context as TeraContext, Tera};

mod codegen;
mod config;
mod sanitize;
#[cfg(test)]
mod tests;
mod types;

use codegen::PackageTemplateContext;
pub use config::PythonPackageConfig;

/// A Python SDK generator.
///
/// Implements [`IrEmitter`]: construct it with [`PythonGenerator::new`] then
/// call [`IrEmitter::generate`] or the individual `emit_*` methods.
pub struct PythonGenerator {
    pub config: PythonPackageConfig,
    pub(crate) tera: Tera,
    /// Extra user-supplied templates discovered at construction time.
    pub(crate) extra_package_templates: Vec<(String, PathBuf)>,
}

impl PythonGenerator {
    /// Build a generator from `config`, compiling the Tera template engine.
    pub fn new(config: &PythonPackageConfig) -> Result<Self> {
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

const TEMPLATE_PYPROJECT: &str = "package/pyproject.toml.tera";
const TEMPLATE_README: &str = "package/README.md.tera";
const TEMPLATE_INIT: &str = "package/__init__.py.tera";
const TEMPLATE_MODELS: &str = "package/models.py.tera";
const TEMPLATE_CLIENT: &str = "package/client.py.tera";
const TEMPLATE_MODEL_CLASS: &str = "partials/model_class.py.tera";
const TEMPLATE_CLIENT_CLASS: &str = "partials/client_class.py.tera";
const TEMPLATE_TAG_CLIENT_CLASS: &str = "partials/tag_client_class.py.tera";
const TEMPLATE_CLIENT_METHOD: &str = "partials/client_method.py.tera";

const BUILTIN_TEMPLATES: &[(&str, &str)] = &[
    (
        TEMPLATE_PYPROJECT,
        include_str!("../templates/package/pyproject.toml.tera"),
    ),
    (
        TEMPLATE_README,
        include_str!("../templates/package/README.md.tera"),
    ),
    (
        TEMPLATE_INIT,
        include_str!("../templates/package/__init__.py.tera"),
    ),
    (
        TEMPLATE_MODELS,
        include_str!("../templates/package/models.py.tera"),
    ),
    (
        TEMPLATE_CLIENT,
        include_str!("../templates/package/client.py.tera"),
    ),
    (
        TEMPLATE_MODEL_CLASS,
        include_str!("../templates/partials/model_class.py.tera"),
    ),
    (
        TEMPLATE_CLIENT_CLASS,
        include_str!("../templates/partials/client_class.py.tera"),
    ),
    (
        TEMPLATE_TAG_CLIENT_CLASS,
        include_str!("../templates/partials/tag_client_class.py.tera"),
    ),
    (
        TEMPLATE_CLIENT_METHOD,
        include_str!("../templates/partials/client_method.py.tera"),
    ),
];

const OVERRIDABLE_TEMPLATES: &[&str] = &[
    TEMPLATE_PYPROJECT,
    TEMPLATE_README,
    TEMPLATE_INIT,
    TEMPLATE_MODELS,
    TEMPLATE_CLIENT,
    TEMPLATE_MODEL_CLASS,
    TEMPLATE_CLIENT_CLASS,
    TEMPLATE_TAG_CLIENT_CLASS,
    TEMPLATE_CLIENT_METHOD,
];

/// Generate a Python client package from an IR snapshot.
///
/// Convenience wrapper around [`PythonGenerator::new`] + [`IrEmitter::generate`].
pub fn generate_python_package(
    ir: &CoreIr,
    config: &PythonPackageConfig,
) -> Result<Vec<GeneratedFile>> {
    PythonGenerator::new(config)?.generate(ir)
}

impl IrEmitter for PythonGenerator {
    fn emit_model(&self, _ir: &CoreIr, model: &Model) -> Result<String> {
        codegen::emit_model(&self.tera, model)
    }

    fn emit_operation(&self, _ir: &CoreIr, operation: &Operation) -> Result<String> {
        codegen::emit_operation(&self.tera, operation)
    }

    fn generate(&self, ir: &CoreIr) -> Result<Vec<GeneratedFile>> {
        assemble_python_files(&self.tera, &self.config, ir, &self.extra_package_templates)
    }
}

pub(crate) fn assemble_python_files(
    tera: &Tera,
    config: &PythonPackageConfig,
    ir: &CoreIr,
    extra_package_templates: &[(String, PathBuf)],
) -> Result<Vec<GeneratedFile>> {
    let package_dir = PathBuf::from("src").join(&config.package_name);
    let package_context = PackageTemplateContext::from_ir(ir, config);
    let mut template_context = TeraContext::new();
    template_context.insert("package", &package_context);

    let mut files = vec![
        GeneratedFile {
            path: PathBuf::from("pyproject.toml"),
            contents: tera
                .render(TEMPLATE_PYPROJECT, &template_context)
                .context("failed to render pyproject template")?,
        },
        GeneratedFile {
            path: PathBuf::from("README.md"),
            contents: tera
                .render(TEMPLATE_README, &template_context)
                .context("failed to render README template")?,
        },
        GeneratedFile {
            path: package_dir.join("__init__.py"),
            contents: tera
                .render(TEMPLATE_INIT, &template_context)
                .context("failed to render package __init__ template")?,
        },
        GeneratedFile {
            path: package_dir.join("models.py"),
            contents: tera
                .render(TEMPLATE_MODELS, &template_context)
                .context("failed to render models template")?,
        },
        GeneratedFile {
            path: package_dir.join("client.py"),
            contents: tera
                .render(TEMPLATE_CLIENT, &template_context)
                .context("failed to render client template")?,
        },
        GeneratedFile {
            path: package_dir.join("py.typed"),
            contents: String::new(),
        },
    ];

    for (template_name, output_path) in extra_package_templates {
        let contents = tera
            .render(template_name, &template_context)
            .with_context(|| format!("failed to render extra template `{template_name}`"))?;
        files.push(GeneratedFile { path: output_path.clone(), contents });
    }

    Ok(files)
}
