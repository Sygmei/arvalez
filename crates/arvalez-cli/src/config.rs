use std::{fs, path::PathBuf};

use anyhow::{Context, Result};
use arvalez_target_core::{CommonConfig as PythonCommonConfig, PackageConfig as PackageMetadata};
use arvalez_target_go::GoPackageConfig;
use arvalez_target_nushell::NushellPackageConfig;
use arvalez_target_python::TargetConfig as PythonTargetConfig;
use arvalez_target_typescript::TypeScriptPackageConfig;
use serde::Deserialize;

// ── App config ────────────────────────────────────────────────────────────────

#[derive(Debug, Default, Deserialize)]
pub(crate) struct AppConfig {
    #[serde(default)]
    pub(crate) common: AppCommonConfig,
    #[serde(default)]
    pub(crate) target: TargetSection,
}

// ── Common section ────────────────────────────────────────────────────────────

#[derive(Debug, Default, Deserialize)]
pub(crate) struct AppCommonConfig {
    #[serde(default)]
    pub(crate) disabled: bool,
    pub(crate) template_dir: Option<PathBuf>,
    #[serde(default)]
    pub(crate) output: CommonOutputConfig,
    #[serde(default)]
    pub(crate) package: CommonPackageConfig,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct CommonOutputConfig {
    pub(crate) directory: Option<PathBuf>,
    #[serde(default)]
    pub(crate) group_by_tag: bool,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct CommonPackageConfig {
    pub(crate) name: Option<String>,
    pub(crate) version: Option<String>,
    pub(crate) description: Option<String>,
}

// ── Shared per-target config ──────────────────────────────────────────────────

/// Output settings that can appear under `[target.<name>.output]`.
/// Any field set here overrides the corresponding `[common.output]` field.
#[derive(Debug, Default, Deserialize)]
pub(crate) struct TargetOutputConfig {
    pub(crate) directory: Option<PathBuf>,
    pub(crate) group_by_tag: Option<bool>,
}

/// Package metadata that can appear under `[target.<name>.package]`.
/// Any field set here overrides the corresponding `[common.package]` field.
#[derive(Debug, Default, Deserialize)]
pub(crate) struct TargetPackageConfig {
    pub(crate) name: Option<String>,
    pub(crate) version: Option<String>,
}

/// Fields present at the root of every `[target.<name>]` table.
#[derive(Debug, Default, Deserialize)]
pub(crate) struct TargetConfig {
    #[serde(default)]
    pub(crate) disabled: bool,
    pub(crate) template_dir: Option<PathBuf>,
    #[serde(default)]
    pub(crate) output: TargetOutputConfig,
    #[serde(default)]
    pub(crate) package: TargetPackageConfig,
}

impl TargetConfig {
    /// Resolve the effective output directory for this target.
    /// Precedence: CLI arg > target.output.directory > common.output.directory > default subdir.
    pub(crate) fn resolve_output_directory(
        &self,
        cli_override: Option<PathBuf>,
        common: &CommonOutputConfig,
        default_subdir: &str,
    ) -> PathBuf {
        cli_override
            .or_else(|| self.output.directory.clone())
            .or_else(|| common.directory.clone())
            .unwrap_or_else(|| PathBuf::from("generated").join(default_subdir))
    }

    /// Resolve the effective group_by_tag flag.
    /// Precedence: CLI flag > target.output.group_by_tag > common.output.group_by_tag.
    pub(crate) fn resolve_group_by_tag(&self, cli_flag: bool, common: &CommonOutputConfig) -> bool {
        cli_flag || self.output.group_by_tag.unwrap_or(common.group_by_tag)
    }

    /// Resolve the effective package version.
    /// Precedence: CLI arg > target.package.version > common.package.version > "0.1.0".
    pub(crate) fn resolve_version(
        &self,
        cli_override: Option<String>,
        common: &CommonPackageConfig,
    ) -> String {
        cli_override
            .or_else(|| self.package.version.clone())
            .or_else(|| common.version.clone())
            .unwrap_or_else(|| "0.1.0".into())
    }

    /// Resolve the effective package name.
    /// Precedence: CLI arg > target.package.name > common.package.name.
    pub(crate) fn resolve_package_name(
        &self,
        cli_override: Option<String>,
        common: &CommonPackageConfig,
        fallback: &str,
    ) -> String {
        cli_override
            .or_else(|| self.package.name.clone())
            .or_else(|| common.name.clone())
            .unwrap_or_else(|| fallback.into())
    }

    /// Resolve the effective template directory.
    /// Precedence: CLI arg > target.template_dir > common.template_dir.
    pub(crate) fn resolve_template_dir(
        &self,
        cli_override: Option<PathBuf>,
        common_template_dir: Option<PathBuf>,
    ) -> Option<PathBuf> {
        cli_override
            .or_else(|| self.template_dir.clone())
            .or(common_template_dir)
    }
}

/// Returns whether a target is enabled after applying the global common flag.
pub(crate) fn is_target_enabled(common: &AppCommonConfig, target_disabled: bool) -> bool {
    !common.disabled && !target_disabled
}

// ── Target-specific extensions ────────────────────────────────────────────────

#[derive(Debug, Default, Deserialize)]
pub(crate) struct GoTargetConfig {
    #[serde(flatten)]
    pub(crate) base: TargetConfig,
    pub(crate) module_path: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct NushellTargetConfig {
    #[serde(flatten)]
    pub(crate) base: TargetConfig,
    pub(crate) module_name: Option<String>,
    pub(crate) default_base_url: Option<String>,
}

// ── Target section ────────────────────────────────────────────────────────────

#[derive(Debug, Default, Deserialize)]
pub(crate) struct TargetSection {
    #[serde(default)]
    pub(crate) go: GoTargetConfig,
    #[serde(default)]
    pub(crate) python: TargetConfig,
    #[serde(default)]
    pub(crate) pythonmini: TargetConfig,
    #[serde(default)]
    pub(crate) typescript: TargetConfig,
    #[serde(default)]
    pub(crate) nushell: NushellTargetConfig,
}

// ── Loading ───────────────────────────────────────────────────────────────────

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

// ── Output directory helpers ──────────────────────────────────────────────────

/// Resolves the root output directory from the CLI override or `common.output.directory`.
pub(crate) fn resolve_output_root(
    config_file: &AppConfig,
    output_directory: Option<PathBuf>,
) -> PathBuf {
    output_directory
        .or_else(|| config_file.common.output.directory.clone())
        .unwrap_or_else(|| PathBuf::from("generated"))
}

/// Resolves the output directory for a specific target.
/// Delegates to `TargetConfig::resolve_output_directory`.
pub(crate) fn resolve_target_output_directory(
    config_file: &AppConfig,
    target: &TargetConfig,
    output_directory: Option<PathBuf>,
    target_dir_name: &str,
) -> PathBuf {
    target.resolve_output_directory(
        output_directory,
        &config_file.common.output,
        target_dir_name,
    )
}

// ── Per-target config resolvers ───────────────────────────────────────────────

pub(crate) fn resolve_go_config(
    config_file: &AppConfig,
    module_path: Option<String>,
    package_name: Option<String>,
    template_dir: Option<PathBuf>,
    group_by_tag: bool,
    output_version: Option<String>,
) -> GoPackageConfig {
    let target = &config_file.target.go;
    let common = &config_file.common;

    let module_path = module_path
        .or_else(|| target.module_path.clone())
        .unwrap_or_else(|| "github.com/arvalez/client".into());
    let resolved_package_name = target
        .base
        .resolve_package_name(package_name, &common.package, "");
    let template_dir = target
        .base
        .resolve_template_dir(template_dir, config_file.common.template_dir.clone());
    let version = target.base.resolve_version(output_version, &common.package);
    let effective_group_by_tag = target
        .base
        .resolve_group_by_tag(group_by_tag, &common.output);

    let mut config = GoPackageConfig::new(module_path)
        .with_version(version)
        .with_template_dir(template_dir)
        .with_group_by_tag(effective_group_by_tag);
    if !resolved_package_name.is_empty() {
        config = config.with_package_name(resolved_package_name);
    }
    config
}

pub(crate) fn resolve_python_config(
    config_file: &AppConfig,
    package_name: Option<String>,
    template_dir: Option<PathBuf>,
    group_by_tag: bool,
    output_version: Option<String>,
) -> (PythonCommonConfig, PythonTargetConfig, Option<PathBuf>) {
    let target = &config_file.target.python;
    let common = &config_file.common;

    let package_name = target.resolve_package_name(package_name, &common.package, "arvalez_client");
    let template_dir =
        target.resolve_template_dir(template_dir, config_file.common.template_dir.clone());
    let version = target.resolve_version(output_version, &common.package);
    let effective_group_by_tag = target.resolve_group_by_tag(group_by_tag, &common.output);

    let common_cfg = PythonCommonConfig {
        package: PackageMetadata {
            name: package_name,
            version,
            description: common.package.description.clone(),
        },
    };
    let target_cfg = PythonTargetConfig {
        group_by_tag: effective_group_by_tag,
    };
    (common_cfg, target_cfg, template_dir)
}

pub(crate) fn resolve_typescript_config(
    config_file: &AppConfig,
    package_name: Option<String>,
    template_dir: Option<PathBuf>,
    group_by_tag: bool,
    output_version: Option<String>,
) -> TypeScriptPackageConfig {
    let target = &config_file.target.typescript;
    let common = &config_file.common;

    let package_name =
        target.resolve_package_name(package_name, &common.package, "@arvalez/client");
    let template_dir =
        target.resolve_template_dir(template_dir, config_file.common.template_dir.clone());
    let version = target.resolve_version(output_version, &common.package);
    let effective_group_by_tag = target.resolve_group_by_tag(group_by_tag, &common.output);

    TypeScriptPackageConfig::new(package_name)
        .with_version(version)
        .with_template_dir(template_dir)
        .with_group_by_tag(effective_group_by_tag)
}

pub(crate) fn resolve_nushell_config(
    config_file: &AppConfig,
    module_name: Option<String>,
    template_dir: Option<PathBuf>,
    default_base_url: Option<String>,
    group_by_tag: bool,
    output_version: Option<String>,
) -> NushellPackageConfig {
    let target = &config_file.target.nushell;
    let common = &config_file.common;

    let module_name = module_name
        .or_else(|| target.module_name.clone())
        .or_else(|| common.package.name.clone())
        .unwrap_or_else(|| "arvalez-client".into());
    let template_dir = target
        .base
        .resolve_template_dir(template_dir, config_file.common.template_dir.clone());
    let version = target.base.resolve_version(output_version, &common.package);
    let base_url = default_base_url
        .or_else(|| target.default_base_url.clone())
        .unwrap_or_default();
    let effective_group_by_tag = target
        .base
        .resolve_group_by_tag(group_by_tag, &common.output);

    NushellPackageConfig::new(module_name)
        .with_version(version)
        .with_template_dir(template_dir)
        .with_default_base_url(base_url)
        .with_group_by_tag(effective_group_by_tag)
}
