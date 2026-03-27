use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::{Command as ProcessCommand, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use arvalez_ir::{CoreIr, validate_ir};
use arvalez_openapi::{LoadOpenApiOptions, OpenApiLoadResult, load_openapi_to_ir_with_options};
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
use clap::{Parser, Subcommand};
use rayon::{ThreadPoolBuilder, prelude::*};
use serde::{Deserialize, Serialize};
use tempfile::TempDir;

const CORPUS_WORKER_STACK_SIZE_BYTES: usize = 16 * 1024 * 1024;

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
    TestApisGuru {
        #[arg(long, default_value = "https://github.com/APIs-guru/openapi-directory.git")]
        repository: String,
        #[arg(long, default_value = "main")]
        reference: String,
        #[arg(long, default_value = "arvalez.toml")]
        config: PathBuf,
        #[arg(long)]
        checkout_directory: Option<PathBuf>,
        #[arg(long = "output-directory")]
        output_directory: Option<PathBuf>,
        #[arg(long)]
        report_directory: Option<PathBuf>,
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
        #[arg(long)]
        limit: Option<usize>,
        #[arg(long)]
        jobs: Option<usize>,
    },
}

#[derive(Debug, Default, Deserialize)]
struct AppConfig {
    #[serde(default)]
    output: OutputConfig,
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

#[derive(Debug, Serialize, Deserialize)]
struct CorpusReport {
    generated_at_unix_seconds: u64,
    repository: String,
    reference: String,
    total_specs: usize,
    passed_specs: usize,
    failed_specs: usize,
    summary: CorpusFailureSummary,
    results: Vec<CorpusSpecResult>,
}

#[derive(Debug, Serialize, Deserialize)]
struct CorpusSpecResult {
    spec: String,
    warning_count: usize,
    targets: Vec<CorpusTargetResult>,
    failure: Option<CorpusFailure>,
}

#[derive(Debug, Serialize, Deserialize)]
struct CorpusTargetResult {
    name: String,
    generated_files: usize,
    failure: Option<CorpusFailure>,
}

#[derive(Debug, Serialize, Deserialize)]
struct CorpusFailure {
    kind: String,
    feature: String,
    pointer: Option<String>,
    target: Option<String>,
    message: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct CorpusFailureSummary {
    total_failures: usize,
    by_kind: BTreeMap<String, usize>,
    by_kind_and_feature: BTreeMap<String, usize>,
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
        Command::TestApisGuru {
            repository,
            reference,
            config,
            checkout_directory,
            output_directory,
            report_directory,
            ignore_unhandled,
            no_go,
            no_python,
            no_typescript,
            output_version,
            limit,
            jobs,
        } => {
            let config_file = load_optional_config(&config)?;
            let options = CorpusTestOptions {
                repository,
                reference,
                checkout_directory,
                output_directory,
                report_directory,
                ignore_unhandled,
                no_go,
                no_python,
                no_typescript,
                output_version,
                limit,
                jobs,
            };
            run_apis_guru_corpus_test(&config_file, &options)?;
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

struct CorpusTestOptions {
    repository: String,
    reference: String,
    checkout_directory: Option<PathBuf>,
    output_directory: Option<PathBuf>,
    report_directory: Option<PathBuf>,
    ignore_unhandled: bool,
    no_go: bool,
    no_python: bool,
    no_typescript: bool,
    output_version: Option<String>,
    limit: Option<usize>,
    jobs: Option<usize>,
}

fn run_apis_guru_corpus_test(config_file: &AppConfig, options: &CorpusTestOptions) -> Result<()> {
    let checkout_directory = if let Some(path) = &options.checkout_directory {
        path.clone()
    } else {
        default_corpus_checkout_directory()?
    };

    prepare_repository_checkout(&options.repository, &options.reference, &checkout_directory)?;

    let mut specs = Vec::new();
    eprintln!(
        "scanning `{}` for supported OpenAPI files...",
        checkout_directory.display()
    );
    collect_openapi_json_files(&checkout_directory, &mut specs)?;
    specs.sort();

    if let Some(limit) = options.limit {
        specs.truncate(limit);
    }

    if specs.is_empty() {
        bail!(
            "no supported OpenAPI files found in `{}` (looked for `openapi.json`, `openapi.yaml`, `swagger.json`, and `swagger.yaml`)",
            checkout_directory.display()
        );
    }

    eprintln!("found {} spec file(s)", specs.len());

    let go_enabled = !options.no_go && !config_file.output.go.disabled;
    let python_enabled = !options.no_python && !config_file.output.python.disabled;
    let typescript_enabled = !options.no_typescript && !config_file.output.typescript.disabled;

    if !go_enabled && !python_enabled && !typescript_enabled {
        bail!("no generation targets enabled");
    }

    let go_config = go_enabled.then(|| {
        resolve_go_config(
            config_file,
            None,
            None,
            None,
            false,
            options.output_version.clone(),
        )
    });
    let python_config = python_enabled.then(|| {
        resolve_python_config(
            config_file,
            None,
            None,
            false,
            options.output_version.clone(),
        )
    });
    let typescript_config = typescript_enabled.then(|| {
        resolve_typescript_config(
            config_file,
            None,
            None,
            false,
            options.output_version.clone(),
        )
    });

    let jobs = resolve_corpus_jobs(options.jobs)?;
    eprintln!("running corpus analysis with {jobs} worker(s)...");
    let completed = AtomicUsize::new(0);
    let total_specs = specs.len();

    let mut indexed_results = ThreadPoolBuilder::new()
        .num_threads(jobs)
        .stack_size(CORPUS_WORKER_STACK_SIZE_BYTES)
        .build()
        .context("failed to build corpus worker pool")?
        .install(|| {
            specs.par_iter()
                .enumerate()
                .map(|(index, spec_path)| {
                    let relative_spec = spec_path
                        .strip_prefix(&checkout_directory)
                        .unwrap_or(spec_path)
                        .to_string_lossy()
                        .replace('\\', "/");

                    let spec_result = run_corpus_spec(
                        spec_path,
                        &relative_spec,
                        options,
                        go_config.as_ref(),
                        python_config.as_ref(),
                        typescript_config.as_ref(),
                    );

                    let failed = spec_result.failure.is_some()
                        || spec_result
                            .targets
                            .iter()
                            .any(|target| target.failure.is_some());
                    let done = completed.fetch_add(1, Ordering::Relaxed) + 1;
                    let status = if failed { "failed" } else { "passed" };
                    eprintln!("[{done}/{total_specs}] {relative_spec} ({status})");

                    (index, spec_result)
                })
                .collect::<Vec<_>>()
        });
    indexed_results.sort_by_key(|(index, _)| *index);
    let results = indexed_results
        .into_iter()
        .map(|(_, result)| result)
        .collect::<Vec<_>>();

    let total_specs = results.len();
    let failed_specs = results
        .iter()
        .filter(|result| {
            result.failure.is_some()
                || result
                    .targets
                    .iter()
                    .any(|target| target.failure.is_some())
        })
        .count();
    let passed_specs = total_specs - failed_specs;
    let summary = summarize_failures(&results);
    let report_data = CorpusReport {
        generated_at_unix_seconds: current_unix_timestamp_seconds()?,
        repository: options.repository.clone(),
        reference: options.reference.clone(),
        total_specs,
        passed_specs,
        failed_specs,
        summary,
        results,
    };

    let report_directory = options
        .report_directory
        .clone()
        .unwrap_or_else(default_report_directory);
    let report_path = write_corpus_report(&report_directory, &report_data)?;
    write_corpus_dashboard(&report_directory)?;
    eprintln!("wrote report to {}", report_path.display());
    eprintln!(
        "updated dashboard at {}",
        report_directory.join("index.html").display()
    );

    eprintln!(
        "completed APIs.guru corpus run: {passed_specs}/{total_specs} specs passed"
    );
    print_failure_summary(&report_data.summary);

    if failed_specs > 0 {
        bail!("{failed_specs} spec(s) failed the APIs.guru corpus run");
    }

    Ok(())
}

fn run_corpus_spec(
    spec_path: &Path,
    relative_spec: &str,
    options: &CorpusTestOptions,
    go_config: Option<&GoPackageConfig>,
    python_config: Option<&PythonPackageConfig>,
    typescript_config: Option<&TypeScriptPackageConfig>,
) -> CorpusSpecResult {
    match load_openapi_to_ir_with_options(spec_path, openapi_options(options.ignore_unhandled)) {
        Ok(OpenApiLoadResult { ir, warnings }) => {
            let mut targets = Vec::new();

            if let Some(go_config) = go_config {
                targets.push(run_go_corpus_target(&ir, relative_spec, options, go_config));
            }
            if let Some(python_config) = python_config {
                targets.push(run_python_corpus_target(
                    &ir,
                    relative_spec,
                    options,
                    python_config,
                ));
            }
            if let Some(typescript_config) = typescript_config {
                targets.push(run_typescript_corpus_target(
                    &ir,
                    relative_spec,
                    options,
                    typescript_config,
                ));
            }

            CorpusSpecResult {
                spec: relative_spec.to_owned(),
                warning_count: warnings.len(),
                targets,
                failure: None,
            }
        }
        Err(error) => CorpusSpecResult {
            spec: relative_spec.to_owned(),
            warning_count: 0,
            targets: Vec::new(),
            failure: Some(classify_failure(&format!("{error:#}"), None)),
        },
    }
}

fn run_go_corpus_target(
    ir: &CoreIr,
    relative_spec: &str,
    options: &CorpusTestOptions,
    config: &GoPackageConfig,
) -> CorpusTargetResult {
    match generate_go_package(ir, config) {
        Ok(files) => write_corpus_target_output(relative_spec, options, "go-client", &files, |output, files| {
            write_go_package(output, files)
        }),
        Err(error) => CorpusTargetResult {
            name: "go".into(),
            generated_files: 0,
            failure: Some(classify_failure(
                &format!("failed to generate go client for `{relative_spec}`: {error:#}"),
                Some("go"),
            )),
        },
    }
}

fn run_python_corpus_target(
    ir: &CoreIr,
    relative_spec: &str,
    options: &CorpusTestOptions,
    config: &PythonPackageConfig,
) -> CorpusTargetResult {
    match generate_python_package(ir, config) {
        Ok(files) => write_corpus_target_output(
            relative_spec,
            options,
            "python-client",
            &files,
            |output, files| write_python_package(output, files),
        ),
        Err(error) => CorpusTargetResult {
            name: "python".into(),
            generated_files: 0,
            failure: Some(classify_failure(
                &format!("failed to generate python client for `{relative_spec}`: {error:#}"),
                Some("python"),
            )),
        },
    }
}

fn run_typescript_corpus_target(
    ir: &CoreIr,
    relative_spec: &str,
    options: &CorpusTestOptions,
    config: &TypeScriptPackageConfig,
) -> CorpusTargetResult {
    match generate_typescript_package(ir, config) {
        Ok(files) => write_corpus_target_output(
            relative_spec,
            options,
            "typescript-client",
            &files,
            |output, files| write_typescript_package(output, files),
        ),
        Err(error) => CorpusTargetResult {
            name: "typescript".into(),
            generated_files: 0,
            failure: Some(classify_failure(
                &format!("failed to generate typescript client for `{relative_spec}`: {error:#}"),
                Some("typescript"),
            )),
        },
    }
}

fn write_corpus_target_output<F, W>(
    relative_spec: &str,
    options: &CorpusTestOptions,
    target_dir_name: &str,
    files: &[F],
    write: W,
) -> CorpusTargetResult
where
    W: for<'a> FnOnce(&'a Path, &[F]) -> Result<()>,
{
    let target_name = target_dir_name.trim_end_matches("-client");
    let temp_output_dir;
    let output_root = if let Some(base_output_directory) = &options.output_directory {
        let relative_parent = Path::new(relative_spec)
            .parent()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("root"));
        base_output_directory.join(relative_parent)
    } else {
        match TempDir::new() {
            Ok(temp_dir) => {
                temp_output_dir = temp_dir;
                temp_output_dir.path().to_path_buf()
            }
            Err(error) => {
                return CorpusTargetResult {
                    name: target_name.to_owned(),
                    generated_files: files.len(),
                    failure: Some(classify_failure(
                        &format!(
                            "failed to create temp output directory for `{relative_spec}`: {error:#}"
                        ),
                        Some(target_name),
                    )),
                };
            }
        }
    };

    let target_output = output_root.join(target_dir_name);
    match write(&target_output, files) {
        Ok(()) => CorpusTargetResult {
            name: target_name.to_owned(),
            generated_files: files.len(),
            failure: None,
        },
        Err(error) => CorpusTargetResult {
            name: target_name.to_owned(),
            generated_files: files.len(),
            failure: Some(classify_failure(
                &format!(
                    "failed to write generated files for `{relative_spec}` into `{}`: {error:#}",
                    target_output.display()
                ),
                Some(target_name),
            )),
        },
    }
}

fn clone_repository(repository: &str, reference: &str, checkout_directory: &Path) -> Result<()> {
    if checkout_directory.exists() {
        bail!(
            "checkout directory `{}` already exists",
            checkout_directory.display()
        );
    }

    if let Some(parent) = checkout_directory.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "failed to create checkout parent directory `{}`",
                parent.display()
            )
        })?;
    }

    eprintln!(
        "cloning `{repository}` at `{reference}` into `{}`...",
        checkout_directory.display()
    );

    let status = ProcessCommand::new("git")
        .args(["clone", "--depth", "1", "--branch", reference, "--single-branch"])
        .arg(repository)
        .arg(checkout_directory)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .with_context(|| format!("failed to spawn git clone for `{repository}`"))?;

    if status.success() {
        Ok(())
    } else {
        bail!("git clone failed for `{repository}`")
    }
}

fn update_repository_checkout(repository: &str, reference: &str, checkout_directory: &Path) -> Result<()> {
    if !checkout_directory.join(".git").exists() {
        bail!(
            "checkout directory `{}` exists but is not a git repository",
            checkout_directory.display()
        );
    }

    eprintln!(
        "reusing cached checkout `{}` and refreshing `{reference}`...",
        checkout_directory.display()
    );

    run_git_command(
        ProcessCommand::new("git")
            .arg("-C")
            .arg(checkout_directory)
            .args(["remote", "set-url", "origin", repository]),
        "git remote set-url failed",
    )?;
    run_git_command(
        ProcessCommand::new("git")
            .arg("-C")
            .arg(checkout_directory)
            .args(["fetch", "--depth", "1", "origin", reference]),
        "git fetch failed",
    )?;
    run_git_command(
        ProcessCommand::new("git")
            .arg("-C")
            .arg(checkout_directory)
            .args(["reset", "--hard", "FETCH_HEAD"]),
        "git reset failed",
    )?;
    run_git_command(
        ProcessCommand::new("git")
            .arg("-C")
            .arg(checkout_directory)
            .args(["clean", "-fd"]),
        "git clean failed",
    )?;

    Ok(())
}

fn prepare_repository_checkout(repository: &str, reference: &str, checkout_directory: &Path) -> Result<()> {
    if checkout_directory.exists() {
        update_repository_checkout(repository, reference, checkout_directory)
    } else {
        clone_repository(repository, reference, checkout_directory)
    }
}

fn default_corpus_checkout_directory() -> Result<PathBuf> {
    let cwd = std::env::current_dir().context("failed to determine current working directory")?;
    Ok(cwd
        .join(".arvalez")
        .join("corpus")
        .join("openapi-directory"))
}

fn run_git_command(command: &mut ProcessCommand, context_message: &str) -> Result<()> {
    let status = command
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .with_context(|| context_message.to_owned())?;

    if status.success() {
        Ok(())
    } else {
        bail!("{context_message}")
    }
}

fn collect_openapi_json_files(root: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(root)
        .with_context(|| format!("failed to read directory `{}`", root.display()))?
    {
        let entry = entry.with_context(|| {
            format!("failed to read entry from directory `{}`", root.display())
        })?;
        let path = entry.path();
        let file_type = entry.file_type().with_context(|| {
            format!("failed to read file type for `{}`", path.display())
        })?;

        if file_type.is_dir() {
            if entry.file_name() == ".git" {
                continue;
            }
            collect_openapi_json_files(&path, files)?;
        } else if file_type.is_file() && is_supported_openapi_filename(&entry.file_name()) {
            files.push(path);
        }
    }

    Ok(())
}

fn is_supported_openapi_filename(file_name: &std::ffi::OsStr) -> bool {
    matches!(
        file_name.to_str(),
        Some("openapi.json" | "openapi.yaml" | "swagger.json" | "swagger.yaml")
    )
}

fn current_unix_timestamp_seconds() -> Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before the unix epoch")?
        .as_secs())
}

fn resolve_corpus_jobs(requested_jobs: Option<usize>) -> Result<usize> {
    if let Some(jobs) = requested_jobs {
        if jobs == 0 {
            bail!("--jobs must be at least 1");
        }
        return Ok(jobs);
    }

    Ok(std::thread::available_parallelism()
        .context("failed to determine available parallelism")?
        .get())
}

fn default_report_directory() -> PathBuf {
    PathBuf::from("reports").join("apis-guru")
}

fn write_corpus_report(report_directory: &Path, report: &CorpusReport) -> Result<PathBuf> {
    fs::create_dir_all(report_directory).with_context(|| {
        format!(
            "failed to create report directory `{}`",
            report_directory.display()
        )
    })?;

    let report_path = report_directory.join(format!(
        "apis-guru-{}.json",
        report.generated_at_unix_seconds
    ));
    fs::write(&report_path, serde_json::to_vec_pretty(report)?)
        .with_context(|| format!("failed to write report `{}`", report_path.display()))?;
    Ok(report_path)
}

fn write_corpus_dashboard(report_directory: &Path) -> Result<()> {
    let reports = load_corpus_reports(report_directory)?;
    let latest_report = reports.last();
    let dashboard = render_corpus_dashboard(report_directory, &reports, latest_report);
    let dashboard_path = report_directory.join("index.html");
    fs::write(&dashboard_path, dashboard)
        .with_context(|| format!("failed to write dashboard `{}`", dashboard_path.display()))?;
    Ok(())
}

fn load_corpus_reports(report_directory: &Path) -> Result<Vec<CorpusReport>> {
    let mut reports = Vec::new();

    if !report_directory.exists() {
        return Ok(reports);
    }

    let mut report_paths = Vec::new();
    for entry in fs::read_dir(report_directory).with_context(|| {
        format!(
            "failed to read report directory `{}`",
            report_directory.display()
        )
    })? {
        let entry = entry.with_context(|| {
            format!(
                "failed to read entry from report directory `{}`",
                report_directory.display()
            )
        })?;
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) == Some("json") {
            report_paths.push(path);
        }
    }

    report_paths.sort();

    for path in report_paths {
        let raw = fs::read(&path)
            .with_context(|| format!("failed to read report `{}`", path.display()))?;
        let report: CorpusReport = serde_json::from_slice(&raw)
            .with_context(|| format!("failed to parse report `{}`", path.display()))?;
        reports.push(report);
    }

    reports.sort_by_key(|report| report.generated_at_unix_seconds);
    Ok(reports)
}

fn render_corpus_dashboard(
    report_directory: &Path,
    reports: &[CorpusReport],
    latest_report: Option<&CorpusReport>,
) -> String {
    let reports_json = serde_json::to_string(reports).expect("reports should serialize");
    let latest_title = latest_report
        .map(|report| {
            format!(
                "{} / {} specs passed",
                report.passed_specs, report.total_specs
            )
        })
        .unwrap_or_else(|| "No reports yet".into());
    let report_dir_label = report_directory.display().to_string();

    format!(
        r##"<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>Arvalez Corpus Dashboard</title>
    <style>
      :root {{
        color-scheme: light;
        --bg: #f7f4ec;
        --panel: #fffdf7;
        --ink: #1f2933;
        --muted: #52606d;
        --line: #d9d0bf;
        --accent: #0f766e;
        --accent-soft: #d9f3ef;
        --danger: #b42318;
        --danger-soft: #fde8e8;
      }}
      * {{ box-sizing: border-box; }}
      body {{
        margin: 0;
        font-family: "Iowan Old Style", "Palatino Linotype", Georgia, serif;
        background:
          radial-gradient(circle at top left, rgba(15, 118, 110, 0.12), transparent 28rem),
          linear-gradient(180deg, #f9f5ea 0%, var(--bg) 100%);
        color: var(--ink);
      }}
      main {{
        max-width: 1200px;
        margin: 0 auto;
        padding: 2rem 1.25rem 3rem;
      }}
      h1, h2 {{ margin: 0 0 0.75rem; }}
      p {{ color: var(--muted); }}
      .hero {{
        display: grid;
        gap: 1rem;
        grid-template-columns: repeat(auto-fit, minmax(14rem, 1fr));
        margin-bottom: 1.5rem;
      }}
      .card {{
        background: color-mix(in srgb, var(--panel) 94%, white);
        border: 1px solid var(--line);
        border-radius: 1rem;
        padding: 1rem 1.1rem;
        box-shadow: 0 10px 30px rgba(31, 41, 51, 0.06);
      }}
      .metric {{
        font-size: 2rem;
        line-height: 1;
        margin-top: 0.3rem;
      }}
      .progress-shell {{
        margin-top: 0.75rem;
        height: 0.9rem;
        border-radius: 999px;
        background: #ece6d7;
        overflow: hidden;
      }}
      .progress-bar {{
        height: 100%;
        background: linear-gradient(90deg, #0f766e 0%, #14b8a6 100%);
      }}
      .grid {{
        display: grid;
        gap: 1rem;
        grid-template-columns: 1.5fr 1fr;
      }}
      .chart {{
        width: 100%;
        height: 280px;
      }}
      .axis {{
        stroke: #cabfae;
        stroke-width: 1;
      }}
      .series {{
        fill: none;
        stroke: var(--accent);
        stroke-width: 3;
        stroke-linecap: round;
        stroke-linejoin: round;
      }}
      .point {{
        fill: #fff;
        stroke: var(--accent);
        stroke-width: 2;
      }}
      table {{
        width: 100%;
        border-collapse: collapse;
        font-size: 0.95rem;
      }}
      th, td {{
        padding: 0.65rem 0.5rem;
        border-bottom: 1px solid var(--line);
        text-align: left;
        vertical-align: top;
      }}
      th {{
        color: var(--muted);
        font-weight: 600;
      }}
      .pill {{
        display: inline-block;
        padding: 0.2rem 0.55rem;
        border-radius: 999px;
        font-size: 0.8rem;
        background: var(--accent-soft);
        color: var(--accent);
      }}
      .pill.danger {{
        background: var(--danger-soft);
        color: var(--danger);
      }}
      code {{
        font-family: "SFMono-Regular", ui-monospace, Menlo, monospace;
        font-size: 0.84em;
      }}
      @media (max-width: 900px) {{
        .grid {{ grid-template-columns: 1fr; }}
      }}
    </style>
  </head>
  <body>
    <main>
      <div class="card" style="margin-bottom: 1rem;">
        <h1>Arvalez Corpus Dashboard</h1>
        <p>Report directory: <code>{report_dir_label}</code></p>
        <p>{latest_title}</p>
      </div>

      <section class="hero">
        <div class="card">
          <div>Latest Support</div>
          <div id="latest-support" class="metric">-</div>
          <div class="progress-shell"><div id="latest-progress" class="progress-bar" style="width: 0%;"></div></div>
        </div>
        <div class="card">
          <div>Latest Failures</div>
          <div id="latest-failures" class="metric">-</div>
        </div>
        <div class="card">
          <div>Report Count</div>
          <div id="report-count" class="metric">-</div>
        </div>
      </section>

      <section class="grid">
        <div class="card">
          <h2>Support Trend</h2>
          <svg id="trend-chart" class="chart" viewBox="0 0 960 280" preserveAspectRatio="none"></svg>
        </div>
        <div class="card">
          <h2>Latest Top Failures</h2>
          <table id="top-failures-table">
            <thead><tr><th>Kind / Feature</th><th>Count</th></tr></thead>
            <tbody></tbody>
          </table>
        </div>
      </section>

      <section class="card" style="margin-top: 1rem;">
        <h2>Report History</h2>
        <table id="history-table">
          <thead>
            <tr>
              <th>Generated</th>
              <th>Support</th>
              <th>Passed</th>
              <th>Failed</th>
              <th>Total</th>
            </tr>
          </thead>
          <tbody></tbody>
        </table>
      </section>
    </main>
    <script>
      const reports = {reports_json};
      const latest = reports.length > 0 ? reports[reports.length - 1] : null;

      const formatDate = (unixSeconds) => new Date(unixSeconds * 1000).toLocaleString();
      const supportPercent = (report) => report.total_specs === 0 ? 0 : (report.passed_specs / report.total_specs) * 100;

      document.getElementById("report-count").textContent = reports.length.toString();
      if (latest) {{
        const percent = supportPercent(latest);
        document.getElementById("latest-support").textContent = `${{percent.toFixed(1)}}%`;
        document.getElementById("latest-progress").style.width = `${{percent}}%`;
        document.getElementById("latest-failures").textContent = latest.failed_specs.toString();

        const tbody = document.querySelector("#top-failures-table tbody");
        const entries = Object.entries(latest.summary.by_kind_and_feature)
          .sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]))
          .slice(0, 12);
        for (const [key, count] of entries) {{
          const row = document.createElement("tr");
          row.innerHTML = `<td><code>${{key}}</code></td><td>${{count}}</td>`;
          tbody.appendChild(row);
        }}
      }}

      const historyTbody = document.querySelector("#history-table tbody");
      for (const report of [...reports].reverse()) {{
        const percent = supportPercent(report);
        const row = document.createElement("tr");
        row.innerHTML = `
          <td>${{formatDate(report.generated_at_unix_seconds)}}</td>
          <td><span class="pill">${{percent.toFixed(1)}}%</span></td>
          <td>${{report.passed_specs}}</td>
          <td><span class="pill danger">${{report.failed_specs}}</span></td>
          <td>${{report.total_specs}}</td>
        `;
        historyTbody.appendChild(row);
      }}

      const chart = document.getElementById("trend-chart");
      if (reports.length > 0) {{
        const width = 960;
        const height = 280;
        const padLeft = 44;
        const padRight = 18;
        const padTop = 18;
        const padBottom = 30;
        const plotWidth = width - padLeft - padRight;
        const plotHeight = height - padTop - padBottom;
        const xs = reports.map((_, i) => reports.length === 1 ? padLeft + plotWidth / 2 : padLeft + (plotWidth * i) / (reports.length - 1));
        const ys = reports.map((report) => padTop + plotHeight - (supportPercent(report) / 100) * plotHeight);

        chart.innerHTML = `
          <line class="axis" x1="${{padLeft}}" y1="${{padTop}}" x2="${{padLeft}}" y2="${{padTop + plotHeight}}"></line>
          <line class="axis" x1="${{padLeft}}" y1="${{padTop + plotHeight}}" x2="${{padLeft + plotWidth}}" y2="${{padTop + plotHeight}}"></line>
          <text x="${{padLeft}}" y="${{padTop + 4}}" fill="#52606d" font-size="12">100%</text>
          <text x="${{padLeft}}" y="${{padTop + plotHeight + 20}}" fill="#52606d" font-size="12">0%</text>
        `;

        const path = xs.map((x, i) => `${{i === 0 ? "M" : "L"}} ${{x}} ${{ys[i]}}`).join(" ");
        const pathEl = document.createElementNS("http://www.w3.org/2000/svg", "path");
        pathEl.setAttribute("d", path);
        pathEl.setAttribute("class", "series");
        chart.appendChild(pathEl);

        reports.forEach((report, i) => {{
          const point = document.createElementNS("http://www.w3.org/2000/svg", "circle");
          point.setAttribute("cx", xs[i]);
          point.setAttribute("cy", ys[i]);
          point.setAttribute("r", 4);
          point.setAttribute("class", "point");
          point.setAttribute("title", `${{formatDate(report.generated_at_unix_seconds)}}: ${{supportPercent(report).toFixed(1)}}%`);
          chart.appendChild(point);
        }});
      }}
    </script>
  </body>
</html>
"##
    )
}

fn summarize_failures(results: &[CorpusSpecResult]) -> CorpusFailureSummary {
    let mut summary = CorpusFailureSummary::default();

    for result in results {
        if let Some(failure) = &result.failure {
            record_failure_summary(&mut summary, failure);
        }
        for target in &result.targets {
            if let Some(failure) = &target.failure {
                record_failure_summary(&mut summary, failure);
            }
        }
    }

    summary
}

fn record_failure_summary(summary: &mut CorpusFailureSummary, failure: &CorpusFailure) {
    summary.total_failures += 1;
    *summary.by_kind.entry(failure.kind.clone()).or_insert(0) += 1;
    *summary
        .by_kind_and_feature
        .entry(format!("{}:{}", failure.kind, failure.feature))
        .or_insert(0) += 1;
}

fn print_failure_summary(summary: &CorpusFailureSummary) {
    if summary.total_failures == 0 {
        return;
    }

    eprintln!("failure summary:");
    for (kind, count) in &summary.by_kind {
        eprintln!("  {kind}: {count}");
    }

    eprintln!("top failure features:");
    let mut features = summary
        .by_kind_and_feature
        .iter()
        .map(|(key, count)| (key.as_str(), *count))
        .collect::<Vec<_>>();
    features.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(right.0)));
    for (key, count) in features.into_iter().take(10) {
        eprintln!("  {key}: {count}");
    }
}

fn classify_failure(message: &str, target: Option<&str>) -> CorpusFailure {
    let pointer = extract_pointer(message);
    let schema_path = extract_between(message, "schema mismatch at `", "`:");

    if let Some(keyword) = extract_between(message, "unknown schema keyword `", "`") {
        return CorpusFailure {
            kind: "unsupported_schema_keyword".into(),
            feature: keyword.to_owned(),
            pointer,
            target: target.map(str::to_owned),
            message: message.to_owned(),
        };
    }

    if let Some(schema_type) = extract_between(message, "unsupported schema type `", "`") {
        return CorpusFailure {
            kind: "unsupported_schema_type".into(),
            feature: schema_type.to_owned(),
            pointer,
            target: target.map(str::to_owned),
            message: message.to_owned(),
        };
    }

    if let Some(reference) = extract_between(message, "unsupported reference `", "`") {
        return CorpusFailure {
            kind: "unsupported_reference".into(),
            feature: reference.to_owned(),
            pointer,
            target: target.map(str::to_owned),
            message: message.to_owned(),
        };
    }

    if let Some(feature) = extract_between(message, "`", "` is not supported yet") {
        return CorpusFailure {
            kind: classify_not_supported_kind(pointer.as_deref(), feature),
            feature: feature.to_owned(),
            pointer,
            target: target.map(str::to_owned),
            message: message.to_owned(),
        };
    }

    if message.contains("schema shape is not supported yet") {
        return CorpusFailure {
            kind: "unsupported_schema_shape".into(),
            feature: pointer
                .as_deref()
                .map(pointer_tail_feature)
                .or_else(|| schema_path.map(normalize_feature))
                .unwrap_or_else(|| "schema_shape".into()),
            pointer,
            target: target.map(str::to_owned),
            message: message.to_owned(),
        };
    }

    if message.contains("failed to parse JSON OpenAPI document")
        || message.contains("failed to parse YAML OpenAPI document")
    {
        return CorpusFailure {
            kind: "invalid_openapi_document".into(),
            feature: schema_path
                .map(normalize_feature)
                .unwrap_or_else(|| "deserialization".into()),
            pointer,
            target: target.map(str::to_owned),
            message: message.to_owned(),
        };
    }

    if message.contains("generated IR is invalid") {
        return CorpusFailure {
            kind: "ir_validation_error".into(),
            feature: "invalid_ir".into(),
            pointer,
            target: target.map(str::to_owned),
            message: message.to_owned(),
        };
    }

    if let Some(target_name) = target {
        if message.contains("failed to write generated files") {
            return CorpusFailure {
                kind: "target_write_error".into(),
                feature: format!("{target_name}.write_output"),
                pointer,
                target: Some(target_name.to_owned()),
                message: message.to_owned(),
            };
        }

        if message.contains("failed to generate") {
            return CorpusFailure {
                kind: "target_generation_error".into(),
                feature: format!("{target_name}.generation"),
                pointer,
                target: Some(target_name.to_owned()),
                message: message.to_owned(),
            };
        }
    }

    CorpusFailure {
        kind: "unknown_error".into(),
        feature: "unknown".into(),
        pointer,
        target: target.map(str::to_owned),
        message: message.to_owned(),
    }
}

fn classify_not_supported_kind(pointer: Option<&str>, feature: &str) -> String {
    if matches!(feature, "allOf" | "anyOf" | "oneOf" | "not" | "discriminator" | "const") {
        return "unsupported_schema_keyword".into();
    }

    match pointer {
        Some(value)
            if value.contains("/components/schemas/")
                || value.contains("/properties/")
                || value.ends_with("/schema")
                || value.contains("/items/") =>
        {
            "unsupported_schema_keyword".into()
        }
        Some(value) if value.contains("/parameters/") => "unsupported_parameter_feature".into(),
        Some(value) if value.contains("/responses/") => "unsupported_response_feature".into(),
        Some(value) if value.contains("/requestBody/") => "unsupported_request_body_feature".into(),
        _ => "unsupported_feature".into(),
    }
}

fn extract_pointer(message: &str) -> Option<String> {
    for line in message.lines() {
        let trimmed = line.trim();
        if let Some(pointer) = trimmed.strip_prefix("location: #/") {
            return Some(format!("#/{pointer}"));
        }
    }
    None
}

fn extract_between<'a>(message: &'a str, prefix: &str, suffix: &str) -> Option<&'a str> {
    let start = message.find(prefix)? + prefix.len();
    let rest = &message[start..];
    let end = rest.find(suffix)?;
    Some(&rest[..end])
}

fn pointer_tail_feature(pointer: &str) -> String {
    pointer
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .map(normalize_feature)
        .unwrap_or_else(|| "schema_shape".into())
}

fn normalize_feature(value: &str) -> String {
    value
        .replace("~1", "/")
        .replace("~0", "~")
        .replace('.', "_")
        .replace('/', "_")
        .replace('`', "")
}
