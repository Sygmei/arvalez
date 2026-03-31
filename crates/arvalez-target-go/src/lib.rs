mod codegen;
mod config;
mod sanitize;
mod types;
#[cfg(test)]
mod tests;

use std::path::PathBuf;

use anyhow::{Context, Result};
use arvalez_ir::CoreIr;
pub use arvalez_target_core::GeneratedFile;
pub use arvalez_target_core::write_files as write_go_package;
use arvalez_target_core::load_templates;
use tera::Context as TeraContext;

pub use config::GoPackageConfig;
use codegen::PackageTemplateContext;

const TEMPLATE_GO_MOD: &str = "package/go.mod.tera";
const TEMPLATE_README: &str = "package/README.md.tera";
const TEMPLATE_MODELS: &str = "package/models.go.tera";
const TEMPLATE_CLIENT: &str = "package/client.go.tera";
const TEMPLATE_MODEL_STRUCT: &str = "partials/model_struct.go.tera";
const TEMPLATE_SERVICE: &str = "partials/service.go.tera";
const TEMPLATE_CLIENT_METHOD: &str = "partials/client_method.go.tera";

const BUILTIN_TEMPLATES: &[(&str, &str)] = &[
    (TEMPLATE_GO_MOD, include_str!("../templates/package/go.mod.tera")),
    (TEMPLATE_README, include_str!("../templates/package/README.md.tera")),
    (TEMPLATE_MODELS, include_str!("../templates/package/models.go.tera")),
    (TEMPLATE_CLIENT, include_str!("../templates/package/client.go.tera")),
    (TEMPLATE_MODEL_STRUCT, include_str!("../templates/partials/model_struct.go.tera")),
    (TEMPLATE_SERVICE, include_str!("../templates/partials/service.go.tera")),
    (TEMPLATE_CLIENT_METHOD, include_str!("../templates/partials/client_method.go.tera")),
];

const OVERRIDABLE_TEMPLATES: &[&str] = &[
    TEMPLATE_GO_MOD, TEMPLATE_README, TEMPLATE_MODELS, TEMPLATE_CLIENT,
    TEMPLATE_MODEL_STRUCT, TEMPLATE_SERVICE, TEMPLATE_CLIENT_METHOD,
];

pub fn generate_go_package(ir: &CoreIr, config: &GoPackageConfig) -> Result<Vec<GeneratedFile>> {
    let tera = load_templates(config.template_dir.as_deref(), BUILTIN_TEMPLATES, OVERRIDABLE_TEMPLATES)?;
    let package_context = PackageTemplateContext::from_ir(ir, config, &tera)?;
    let mut context = TeraContext::new();
    context.insert("package", &package_context);
    Ok(vec![
        GeneratedFile { path: PathBuf::from("go.mod"), contents: tera.render(TEMPLATE_GO_MOD, &context).context("failed to render go.mod template")? },
        GeneratedFile { path: PathBuf::from("README.md"), contents: tera.render(TEMPLATE_README, &context).context("failed to render README template")? },
        GeneratedFile { path: PathBuf::from("models.go"), contents: tera.render(TEMPLATE_MODELS, &context).context("failed to render models template")? },
        GeneratedFile { path: PathBuf::from("client.go"), contents: tera.render(TEMPLATE_CLIENT, &context).context("failed to render client template")? },
    ])
}
