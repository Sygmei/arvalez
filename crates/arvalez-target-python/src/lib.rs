use std::path::PathBuf;

use anyhow::{Context, Result};
use arvalez_ir::CoreIr;
pub use arvalez_target_core::GeneratedFile;
pub use arvalez_target_core::write_files as write_python_package;
use arvalez_target_core::load_templates;
use tera::Context as TeraContext;

mod sanitize;
mod types;
mod config;
mod codegen;
#[cfg(test)]
mod tests;

pub use config::PythonPackageConfig;
use codegen::PackageTemplateContext;

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

pub fn generate_python_package(ir: &CoreIr, config: &PythonPackageConfig) -> Result<Vec<GeneratedFile>> {
    let package_dir = PathBuf::from("src").join(&config.package_name);
    let tera = load_templates(config.template_dir.as_deref(), BUILTIN_TEMPLATES, OVERRIDABLE_TEMPLATES)?;
    let package_context = PackageTemplateContext::from_ir(ir, config, &tera)?;
    let mut template_context = TeraContext::new();
    template_context.insert("package", &package_context);

    Ok(vec![
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
    ])
}
