use std::{collections::BTreeMap, fs, path::PathBuf};

use anyhow::{Context, Result, bail};
use arvalez_ir::{CoreIr, Target, validate_ir};
use arvalez_openapi::{LoadOpenApiOptions, OpenApiLoadResult, load_openapi_to_ir_with_options};
use arvalez_plugin_runtime::{WasmPluginDefinition, WasmPluginRunner};
use arvalez_target_go::{
    GoPackageConfig, generate_package as generate_go_package, write_package as write_go_package,
};
use arvalez_target_python::{
    PythonPackageConfig, generate_package as generate_python_package,
    write_package as write_python_package,
};
use arvalez_target_typescript::{
    TypeScriptPackageConfig, generate_package as generate_typescript_package,
    write_package as write_typescript_package,
};
use clap::{Parser, Subcommand, ValueEnum};
use serde::Deserialize;

#[derive(Parser)]
#[command(author, version, about = "Arvalez local development CLI")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    InspectIr {
        #[arg(long, default_value = "fixtures/core_ir.json")]
        ir: PathBuf,
    },
    BuildIr {
        #[arg(long, default_value = "openapi.json")]
        openapi: PathBuf,
        #[arg(long)]
        ignore_unhandled: bool,
    },
    Generate {
        #[arg(long)]
        ir: Option<PathBuf>,
        #[arg(long)]
        openapi: Option<PathBuf>,
        #[arg(long, default_value = "arvalez.toml")]
        config: PathBuf,
        #[arg(long = "output-directory")]
        output_directory: Option<PathBuf>,
        #[arg(long)]
        ignore_unhandled: bool,
        #[arg(long)]
        no_go: bool,
        #[arg(long)]
        no_python: bool,
        #[arg(long)]
        no_typescript: bool,
        #[arg(long)]
        output_version: Option<String>,
    },
    GenerateGo {
        #[arg(long)]
        ir: Option<PathBuf>,
        #[arg(long)]
        openapi: Option<PathBuf>,
        #[arg(long, default_value = "arvalez.toml")]
        config: PathBuf,
        #[arg(long = "output-directory")]
        output_directory: Option<PathBuf>,
        #[arg(long)]
        ignore_unhandled: bool,
        #[arg(long)]
        module_path: Option<String>,
        #[arg(long)]
        package_name: Option<String>,
        #[arg(long)]
        template_dir: Option<PathBuf>,
        #[arg(long)]
        group_by_tag: bool,
        #[arg(long)]
        output_version: Option<String>,
    },
    GeneratePython {
        #[arg(long)]
        ir: Option<PathBuf>,
        #[arg(long)]
        openapi: Option<PathBuf>,
        #[arg(long, default_value = "arvalez.toml")]
        config: PathBuf,
        #[arg(long = "output-directory")]
        output_directory: Option<PathBuf>,
        #[arg(long)]
        ignore_unhandled: bool,
        #[arg(long)]
        package_name: Option<String>,
        #[arg(long)]
        template_dir: Option<PathBuf>,
        #[arg(long)]
        group_by_tag: bool,
        #[arg(long)]
        output_version: Option<String>,
    },
    GenerateTypescript {
        #[arg(long)]
        ir: Option<PathBuf>,
        #[arg(long)]
        openapi: Option<PathBuf>,
        #[arg(long, default_value = "arvalez.toml")]
        config: PathBuf,
        #[arg(long = "output-directory")]
        output_directory: Option<PathBuf>,
        #[arg(long)]
        ignore_unhandled: bool,
        #[arg(long)]
        package_name: Option<String>,
        #[arg(long)]
        template_dir: Option<PathBuf>,
        #[arg(long)]
        group_by_tag: bool,
        #[arg(long)]
        output_version: Option<String>,
    },
    RunPlugin {
        #[arg(long, default_value = "arvalez.toml")]
        config: PathBuf,
        #[arg(long)]
        ir: Option<PathBuf>,
        #[arg(long)]
        openapi: Option<PathBuf>,
        #[arg(long)]
        ignore_unhandled: bool,
        #[arg(long)]
        plugin: String,
        #[arg(long)]
        target: Option<TargetArg>,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum TargetArg {
    Python,
    Go,
    Typescript,
}

impl From<TargetArg> for Target {
    fn from(value: TargetArg) -> Self {
        match value {
            TargetArg::Python => Target::Python,
            TargetArg::Go => Target::Go,
            TargetArg::Typescript => Target::Typescript,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct AppConfig {
    #[serde(default)]
    plugins: BTreeMap<String, PluginConfig>,
    #[serde(default)]
    output: OutputConfig,
}

#[derive(Debug, Deserialize)]
struct PluginConfig {
    path: PathBuf,
    #[serde(default)]
    options: toml::Table,
}

#[derive(Debug, Default, Deserialize)]
struct OutputConfig {
    #[serde(default)]
    group_by_tag: bool,
    version: Option<String>,
    directory: Option<PathBuf>,
    #[serde(default)]
    go: GoConfig,
    #[serde(default)]
    python: PythonConfig,
    #[serde(default)]
    typescript: TypeScriptConfig,
}

#[derive(Debug, Default, Deserialize)]
struct GoConfig {
    #[serde(default)]
    disabled: bool,
    module_path: Option<String>,
    package_name: Option<String>,
    version: Option<String>,
    template_dir: Option<PathBuf>,
    group_by_tag: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
struct PythonConfig {
    #[serde(default)]
    disabled: bool,
    package_name: Option<String>,
    version: Option<String>,
    template_dir: Option<PathBuf>,
    group_by_tag: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
struct TypeScriptConfig {
    #[serde(default)]
    disabled: bool,
    package_name: Option<String>,
    version: Option<String>,
    template_dir: Option<PathBuf>,
    group_by_tag: Option<bool>,
}

impl PluginConfig {
    fn to_runtime_definition(&self) -> Result<WasmPluginDefinition> {
        let options =
            serde_json::to_value(&self.options).context("failed to convert plugin options")?;
        Ok(WasmPluginDefinition {
            path: self.path.clone(),
            options,
        })
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::InspectIr { ir } => {
            let ir = load_ir(&ir)?;
            println!("{}", serde_json::to_string_pretty(&ir)?);
        }
        Command::BuildIr {
            openapi,
            ignore_unhandled,
        } => {
            let OpenApiLoadResult { ir, warnings } =
                load_openapi_to_ir_with_options(&openapi, openapi_options(ignore_unhandled))?;
            print_openapi_warnings(&warnings);
            println!("{}", serde_json::to_string_pretty(&ir)?);
        }
        Command::Generate {
            ir,
            openapi,
            config,
            output_directory,
            ignore_unhandled,
            no_go,
            no_python,
            no_typescript,
            output_version,
        } => {
            let (ir, warnings) = load_input_ir(ir, openapi, ignore_unhandled)?;
            print_openapi_warnings(&warnings);
            let config_file = load_optional_config(&config)?;
            let output_root = resolve_output_root(&config_file, output_directory);

            let go_enabled = !no_go && !config_file.output.go.disabled;
            let python_enabled = !no_python && !config_file.output.python.disabled;
            let typescript_enabled = !no_typescript && !config_file.output.typescript.disabled;

            if !go_enabled && !python_enabled && !typescript_enabled {
                bail!("no generation targets enabled");
            }

            if go_enabled {
                let go_config =
                    resolve_go_config(&config_file, None, None, None, false, output_version.clone());
                let files = generate_go_package(&ir, &go_config)?;
                let output = output_root.join("go-client");
                write_go_package(&output, &files)?;
                eprintln!("generated {} files into {}", files.len(), output.display());
            }

            if python_enabled {
                let python_config =
                    resolve_python_config(&config_file, None, None, false, output_version.clone());
                let files = generate_python_package(&ir, &python_config)?;
                let output = output_root.join("python-client");
                write_python_package(&output, &files)?;
                eprintln!("generated {} files into {}", files.len(), output.display());
            }

            if typescript_enabled {
                let typescript_config = resolve_typescript_config(
                    &config_file,
                    None,
                    None,
                    false,
                    output_version.clone(),
                );
                let files = generate_typescript_package(&ir, &typescript_config)?;
                let output = output_root.join("typescript-client");
                write_typescript_package(&output, &files)?;
                eprintln!("generated {} files into {}", files.len(), output.display());
            }
        }
        Command::GenerateGo {
            ir,
            openapi,
            config,
            output_directory,
            ignore_unhandled,
            module_path,
            package_name,
            template_dir,
            group_by_tag,
            output_version,
        } => {
            let (ir, warnings) = load_input_ir(ir, openapi, ignore_unhandled)?;
            print_openapi_warnings(&warnings);
            let config_file = load_optional_config(&config)?;
            let output = resolve_target_output_directory(&config_file, output_directory, "go-client");
            let go_config = resolve_go_config(
                &config_file,
                module_path,
                package_name,
                template_dir,
                group_by_tag,
                output_version,
            );
            let files = generate_go_package(&ir, &go_config)?;
            write_go_package(&output, &files)?;
            eprintln!("generated {} files into {}", files.len(), output.display());
        }
        Command::GeneratePython {
            ir,
            openapi,
            config,
            output_directory,
            ignore_unhandled,
            package_name,
            template_dir,
            group_by_tag,
            output_version,
        } => {
            let (ir, warnings) = load_input_ir(ir, openapi, ignore_unhandled)?;
            print_openapi_warnings(&warnings);
            let config_file = load_optional_config(&config)?;
            let output =
                resolve_target_output_directory(&config_file, output_directory, "python-client");
            let python_config = resolve_python_config(
                &config_file,
                package_name,
                template_dir,
                group_by_tag,
                output_version,
            );
            let files = generate_python_package(&ir, &python_config)?;
            write_python_package(&output, &files)?;
            eprintln!("generated {} files into {}", files.len(), output.display());
        }
        Command::GenerateTypescript {
            ir,
            openapi,
            config,
            output_directory,
            ignore_unhandled,
            package_name,
            template_dir,
            group_by_tag,
            output_version,
        } => {
            let (ir, warnings) = load_input_ir(ir, openapi, ignore_unhandled)?;
            print_openapi_warnings(&warnings);
            let config_file = load_optional_config(&config)?;
            let output = resolve_target_output_directory(
                &config_file,
                output_directory,
                "typescript-client",
            );
            let typescript_config = resolve_typescript_config(
                &config_file,
                package_name,
                template_dir,
                group_by_tag,
                output_version,
            );
            let files = generate_typescript_package(&ir, &typescript_config)?;
            write_typescript_package(&output, &files)?;
            eprintln!("generated {} files into {}", files.len(), output.display());
        }
        Command::RunPlugin {
            config,
            ir,
            openapi,
            ignore_unhandled,
            plugin,
            target,
        } => {
            let config_data = load_config(&config)?;
            let plugin_config = config_data.plugins.get(&plugin).with_context(|| {
                format!("plugin `{plugin}` is not defined in `{}`", config.display())
            })?;
            let (ir, warnings) = load_input_ir(ir, openapi, ignore_unhandled)?;
            print_openapi_warnings(&warnings);
            let runner = WasmPluginRunner::new()?;
            let response = runner.run(
                &plugin,
                &plugin_config.to_runtime_definition()?,
                target.map(Into::into),
                &ir,
            )?;

            for warning in &response.warnings {
                eprintln!("warning: {warning}");
            }

            println!("{}", serde_json::to_string_pretty(&response.ir)?);
        }
    }

    Ok(())
}

fn load_config(path: &PathBuf) -> Result<AppConfig> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read config `{}`", path.display()))?;
    let config: AppConfig =
        toml::from_str(&raw).with_context(|| format!("failed to parse `{}`", path.display()))?;
    Ok(config)
}

fn load_optional_config(path: &PathBuf) -> Result<AppConfig> {
    if path.exists() {
        load_config(path)
    } else {
        Ok(AppConfig::default())
    }
}

fn load_ir(path: &PathBuf) -> Result<CoreIr> {
    let raw = fs::read(path)
        .with_context(|| format!("failed to read IR fixture `{}`", path.display()))?;
    let ir: CoreIr = serde_json::from_slice(&raw)
        .with_context(|| format!("failed to parse `{}` as CoreIr", path.display()))?;

    if let Err(errors) = validate_ir(&ir) {
        for issue in errors.0 {
            eprintln!("validation error: {}: {}", issue.path, issue.message);
        }
        bail!("IR fixture is invalid");
    }

    Ok(ir)
}

fn resolve_output_root(config_file: &AppConfig, output_directory: Option<PathBuf>) -> PathBuf {
    output_directory
        .or(config_file.output.directory.clone())
        .unwrap_or_else(|| PathBuf::from("generated"))
}

fn resolve_target_output_directory(
    config_file: &AppConfig,
    output_directory: Option<PathBuf>,
    target_dir_name: &str,
) -> PathBuf {
    output_directory.unwrap_or_else(|| resolve_output_root(config_file, None).join(target_dir_name))
}

fn resolve_go_config(
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

fn resolve_python_config(
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

fn resolve_typescript_config(
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

fn load_input_ir(
    ir: Option<PathBuf>,
    openapi: Option<PathBuf>,
    ignore_unhandled: bool,
) -> Result<(CoreIr, Vec<String>)> {
    match (ir, openapi) {
        (Some(ir), None) => Ok((load_ir(&ir)?, Vec::new())),
        (None, Some(openapi)) => {
            let result =
                load_openapi_to_ir_with_options(&openapi, openapi_options(ignore_unhandled))?;
            Ok((result.ir, result.warnings))
        }
        (None, None) => Ok((
            load_ir(&PathBuf::from("fixtures/core_ir.json"))?,
            Vec::new(),
        )),
        (Some(_), Some(_)) => bail!("pass either --ir or --openapi, not both"),
    }
}

fn openapi_options(ignore_unhandled: bool) -> LoadOpenApiOptions {
    LoadOpenApiOptions { ignore_unhandled }
}

fn print_openapi_warnings(warnings: &[String]) {
    for warning in warnings {
        eprintln!("warning: {warning}");
    }
}
