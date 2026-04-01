use std::{
    fs,
    path::PathBuf,
};

use anyhow::{Context, Result};
use arvalez_target_core::CommonConfig;
use arvalez_target_go::GoPackageConfig;
use arvalez_target_nushell::TargetConfig as NushellTargetConfig;
use arvalez_target_python::PythonPackageConfig;
use arvalez_target_typescript::TypeScriptPackageConfig;
use serde::Deserialize;

#[derive(Debug, Default, Deserialize)]
pub(crate) struct AppConfig {
    #[serde(default)]
    pub(crate) output: OutputConfig,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct OutputConfig {
    #[serde(default)]
    pub(crate) group_by_tag: bool,
    pub(crate) version: Option<String>,
    pub(crate) directory: Option<PathBuf>,
    #[serde(default)]
    pub(crate) go: GoConfig,
    #[serde(default)]
    pub(crate) python: PythonConfig,
    #[serde(default)]
    pub(crate) typescript: TypeScriptConfig,
    #[serde(default)]
    pub(crate) nushell: NushellConfig,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct GoConfig {
    #[serde(default)]
    pub(crate) disabled: bool,
    pub(crate) module_path: Option<String>,
    pub(crate) package_name: Option<String>,
    pub(crate) version: Option<String>,
    pub(crate) template_dir: Option<PathBuf>,
    pub(crate) group_by_tag: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct PythonConfig {
    #[serde(default)]
    pub(crate) disabled: bool,
    pub(crate) package_name: Option<String>,
    pub(crate) version: Option<String>,
    pub(crate) template_dir: Option<PathBuf>,
    pub(crate) group_by_tag: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct TypeScriptConfig {
    #[serde(default)]
    pub(crate) disabled: bool,
    pub(crate) package_name: Option<String>,
    pub(crate) version: Option<String>,
    pub(crate) template_dir: Option<PathBuf>,
    pub(crate) group_by_tag: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct NushellConfig {
    #[serde(default)]
    pub(crate) disabled: bool,
    pub(crate) package_name: Option<String>,
    pub(crate) version: Option<String>,
    pub(crate) template_dir: Option<PathBuf>,
    pub(crate) default_base_url: Option<String>,
    pub(crate) group_by_tag: Option<bool>,
}

pub(crate) fn load_config(path: &PathBuf) -> Result<AppConfig> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read config `{}`", path.display()))?;
    let config: AppConfig =
        toml::from_str(&raw).with_context(|| format!("failed to parse `{}`", path.display()))?;
    Ok(config)
}

pub(crate) fn load_optional_config(path: &PathBuf) -> Result<AppConfig> {
    if path.exists() {
        load_config(path)
    } else {
        Ok(AppConfig::default())
    }
}

pub(crate) fn resolve_output_root(
    config_file: &AppConfig,
    output_directory: Option<PathBuf>,
) -> PathBuf {
    output_directory
        .or(config_file.output.directory.clone())
        .unwrap_or_else(|| PathBuf::from("generated"))
}

pub(crate) fn resolve_target_output_directory(
    config_file: &AppConfig,
    output_directory: Option<PathBuf>,
    target_dir_name: &str,
) -> PathBuf {
    output_directory
        .unwrap_or_else(|| resolve_output_root(config_file, None).join(target_dir_name))
}

pub(crate) fn resolve_go_config(
    config_file: &AppConfig,
    module_path: Option<String>,
    package_name: Option<String>,
    template_dir: Option<PathBuf>,
    group_by_tag: bool,
    output_version: Option<String>,
) -> GoPackageConfig {
    let module_path = module_path
        .or(config_file.output.go.module_path.clone())
        .unwrap_or_else(|| "github.com/arvalez/client".into());
    let package_name = package_name.or(config_file.output.go.package_name.clone());
    let template_dir = template_dir.or(config_file.output.go.template_dir.clone());
    let version = output_version
        .or(config_file.output.go.version.clone())
        .or(config_file.output.version.clone())
        .unwrap_or_else(|| "0.1.0".into());
    let effective_group_by_tag = group_by_tag
        || config_file
            .output
            .go
            .group_by_tag
            .unwrap_or(config_file.output.group_by_tag);

    let mut config = GoPackageConfig::new(module_path)
        .with_version(version)
        .with_template_dir(template_dir)
        .with_group_by_tag(effective_group_by_tag);
    if let Some(package_name) = package_name {
        config = config.with_package_name(package_name);
    }
    config
}

pub(crate) fn resolve_python_config(
    config_file: &AppConfig,
    package_name: Option<String>,
    template_dir: Option<PathBuf>,
    group_by_tag: bool,
    output_version: Option<String>,
) -> PythonPackageConfig {
    let package_name = package_name
        .or(config_file.output.python.package_name.clone())
        .unwrap_or_else(|| "arvalez_client".into());
    let template_dir = template_dir.or(config_file.output.python.template_dir.clone());
    let version = output_version
        .or(config_file.output.python.version.clone())
        .or(config_file.output.version.clone())
        .unwrap_or_else(|| "0.1.0".into());
    let effective_group_by_tag = group_by_tag
        || config_file
            .output
            .python
            .group_by_tag
            .unwrap_or(config_file.output.group_by_tag);

    PythonPackageConfig::new(package_name)
        .with_version(version)
        .with_template_dir(template_dir)
        .with_group_by_tag(effective_group_by_tag)
}

pub(crate) fn resolve_typescript_config(
    config_file: &AppConfig,
    package_name: Option<String>,
    template_dir: Option<PathBuf>,
    group_by_tag: bool,
    output_version: Option<String>,
) -> TypeScriptPackageConfig {
    let package_name = package_name
        .or(config_file.output.typescript.package_name.clone())
        .unwrap_or_else(|| "@arvalez/client".into());
    let template_dir = template_dir.or(config_file.output.typescript.template_dir.clone());
    let version = output_version
        .or(config_file.output.typescript.version.clone())
        .or(config_file.output.version.clone())
        .unwrap_or_else(|| "0.1.0".into());
    let effective_group_by_tag = group_by_tag
        || config_file
            .output
            .typescript
            .group_by_tag
            .unwrap_or(config_file.output.group_by_tag);

    TypeScriptPackageConfig::new(package_name)
        .with_version(version)
        .with_template_dir(template_dir)
        .with_group_by_tag(effective_group_by_tag)
}

pub(crate) fn resolve_nushell_config(
    config_file: &AppConfig,
    package_name: Option<String>,
    template_dir: Option<PathBuf>,
    default_base_url: Option<String>,
    group_by_tag: bool,
    output_version: Option<String>,
) -> (Option<PathBuf>, CommonConfig, NushellTargetConfig) {
    let package_name = package_name
        .or(config_file.output.nushell.package_name.clone())
        .unwrap_or_else(|| "arvalez-client".into());
    let template_dir = template_dir.or(config_file.output.nushell.template_dir.clone());
    let version = output_version
        .or(config_file.output.nushell.version.clone())
        .or(config_file.output.version.clone())
        .unwrap_or_else(|| "0.1.0".into());
    let base_url = default_base_url
        .or(config_file.output.nushell.default_base_url.clone())
        .unwrap_or_default();
    let effective_group_by_tag = group_by_tag
        || config_file
            .output
            .nushell
            .group_by_tag
            .unwrap_or(config_file.output.group_by_tag);

    let common = CommonConfig { package_name, version };
    let config = NushellTargetConfig { default_base_url: base_url, group_by_tag: effective_group_by_tag };
    (template_dir, common, config)
}
