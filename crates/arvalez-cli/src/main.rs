use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fs,
    io::{IsTerminal, Write},
    path::{Path, PathBuf},
    process::{Command as ProcessCommand, ExitStatus, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[cfg(unix)]
use std::os::unix::process::ExitStatusExt;

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
use crossterm::{
    event::{self, Event, KeyCode},
    execute,
    terminal::{
        EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
    },
};
use rayon::{ThreadPoolBuilder, prelude::*};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Gauge, List, ListItem, Paragraph},
};
use serde::{Deserialize, Serialize};
use tempfile::TempDir;

const CORPUS_WORKER_STACK_SIZE_BYTES: usize = 16 * 1024 * 1024;
const CORPUS_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(3);
const CORPUS_UI_TICK_INTERVAL: Duration = Duration::from_millis(200);
const CORPUS_UI_RECENT_LIMIT: usize = 8;
const CORPUS_UI_ACTIVE_SAMPLE_LIMIT: usize = 8;
// Some real-world Azure/Codat specs still recurse deeply enough during import
// that the default Rust thread stack and our earlier 64 MiB budget were not
// sufficient. Keep this comfortably high so corpus runs record real importer
// errors instead of aborting on stack overflow.
const OPENAPI_LOAD_STACK_SIZE_BYTES: usize = 256 * 1024 * 1024;

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
        #[arg(long)]
        timings: bool,
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
        #[arg(long)]
        timings: bool,
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
        #[arg(long)]
        timings: bool,
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
        #[arg(long)]
        timings: bool,
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
        #[arg(long)]
        timings: bool,
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
        #[arg(long)]
        ui: bool,
    },
    #[command(hide = true)]
    CorpusSpecWorker {
        #[arg(long)]
        spec_path: PathBuf,
        #[arg(long)]
        relative_spec: String,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pointer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    schema_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    line: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    column: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source_preview: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    target: Option<String>,
    message: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct CorpusFailureSummary {
    total_failures: usize,
    by_kind: BTreeMap<String, usize>,
    by_kind_and_feature: BTreeMap<String, usize>,
}

#[derive(Debug, Clone)]
struct CompletedCorpusSpec {
    spec: String,
    status: &'static str,
}

#[derive(Debug, Default)]
struct CorpusProgressSnapshot {
    active_specs: Vec<String>,
    recent_completed: Vec<CompletedCorpusSpec>,
    passed_specs: usize,
    failed_specs: usize,
}

#[derive(Debug, Default)]
struct CorpusProgressState {
    active_specs: BTreeSet<String>,
    recent_completed: VecDeque<CompletedCorpusSpec>,
    passed_specs: usize,
    failed_specs: usize,
}

struct TimingCollector {
    enabled: bool,
    started_at: Instant,
    phases: Vec<(String, Duration)>,
}

impl TimingCollector {
    fn new(enabled: bool) -> Self {
        Self {
            enabled,
            started_at: Instant::now(),
            phases: Vec::new(),
        }
    }

    fn measure_result<T, F>(&mut self, label: impl Into<String>, task: F) -> Result<T>
    where
        F: FnOnce() -> Result<T>,
    {
        let label = label.into();
        if self.enabled {
            eprintln!("timing: starting {label}");
        }
        let started = Instant::now();
        let value = task();
        if self.enabled {
            let elapsed = started.elapsed();
            eprintln!("timing: {:<20} {}", label, format_duration(elapsed));
            self.phases.push((label, elapsed));
        }
        value
    }

    fn print(&self) {
        if !self.enabled {
            return;
        }

        eprintln!("timings:");
        for (label, duration) in &self.phases {
            eprintln!("  {:<20} {}", label, format_duration(*duration));
        }
        eprintln!("  {:<20} {}", "total", format_duration(self.started_at.elapsed()));
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
            timings,
        } => {
            let mut timing_collector = TimingCollector::new(timings);
            let openapi = openapi.clone();
            let OpenApiLoadResult { ir, warnings } =
                timing_collector.measure_result("openapi_load", move || {
                    run_with_large_stack("build-ir", move || {
                        load_openapi_to_ir_with_options(
                            &openapi,
                            openapi_options(ignore_unhandled, timings),
                        )
                    })
                })?;
            print_openapi_warnings(&warnings);
            let rendered_ir = timing_collector.measure_result("ir_serialize", || {
                Ok(serde_json::to_string_pretty(&ir)?)
            })?;
            println!("{rendered_ir}");
            timing_collector.print();
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
            timings,
        } => {
            let mut timing_collector = TimingCollector::new(timings);
            let (ir, warnings) =
                load_input_ir(ir, openapi, ignore_unhandled, &mut timing_collector)?;
            print_openapi_warnings(&warnings);
            let config_file =
                timing_collector.measure_result("config_load", || load_optional_config(&config))?;
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
                let files =
                    timing_collector.measure_result("go_generate", || generate_go_package(&ir, &go_config))?;
                let output = output_root.join("go-client");
                timing_collector.measure_result("go_write", || write_go_package(&output, &files))?;
                eprintln!("generated {} files into {}", files.len(), output.display());
            }

            if python_enabled {
                let python_config =
                    resolve_python_config(&config_file, None, None, false, output_version.clone());
                let files = timing_collector.measure_result("python_generate", || {
                    generate_python_package(&ir, &python_config)
                })?;
                let output = output_root.join("python-client");
                timing_collector.measure_result("python_write", || {
                    write_python_package(&output, &files)
                })?;
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
                let files = timing_collector.measure_result("typescript_generate", || {
                    generate_typescript_package(&ir, &typescript_config)
                })?;
                let output = output_root.join("typescript-client");
                timing_collector.measure_result("typescript_write", || {
                    write_typescript_package(&output, &files)
                })?;
                eprintln!("generated {} files into {}", files.len(), output.display());
            }
            timing_collector.print();
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
            timings,
        } => {
            let mut timing_collector = TimingCollector::new(timings);
            let (ir, warnings) =
                load_input_ir(ir, openapi, ignore_unhandled, &mut timing_collector)?;
            print_openapi_warnings(&warnings);
            let config_file =
                timing_collector.measure_result("config_load", || load_optional_config(&config))?;
            let output = resolve_target_output_directory(&config_file, output_directory, "go-client");
            let go_config = resolve_go_config(
                &config_file,
                module_path,
                package_name,
                template_dir,
                group_by_tag,
                output_version,
            );
            let files =
                timing_collector.measure_result("go_generate", || generate_go_package(&ir, &go_config))?;
            timing_collector.measure_result("go_write", || write_go_package(&output, &files))?;
            eprintln!("generated {} files into {}", files.len(), output.display());
            timing_collector.print();
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
            timings,
        } => {
            let mut timing_collector = TimingCollector::new(timings);
            let (ir, warnings) =
                load_input_ir(ir, openapi, ignore_unhandled, &mut timing_collector)?;
            print_openapi_warnings(&warnings);
            let config_file =
                timing_collector.measure_result("config_load", || load_optional_config(&config))?;
            let output =
                resolve_target_output_directory(&config_file, output_directory, "python-client");
            let python_config = resolve_python_config(
                &config_file,
                package_name,
                template_dir,
                group_by_tag,
                output_version,
            );
            let files = timing_collector.measure_result("python_generate", || {
                generate_python_package(&ir, &python_config)
            })?;
            timing_collector.measure_result("python_write", || {
                write_python_package(&output, &files)
            })?;
            eprintln!("generated {} files into {}", files.len(), output.display());
            timing_collector.print();
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
            timings,
        } => {
            let mut timing_collector = TimingCollector::new(timings);
            let (ir, warnings) =
                load_input_ir(ir, openapi, ignore_unhandled, &mut timing_collector)?;
            print_openapi_warnings(&warnings);
            let config_file =
                timing_collector.measure_result("config_load", || load_optional_config(&config))?;
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
            let files = timing_collector.measure_result("typescript_generate", || {
                generate_typescript_package(&ir, &typescript_config)
            })?;
            timing_collector.measure_result("typescript_write", || {
                write_typescript_package(&output, &files)
            })?;
            eprintln!("generated {} files into {}", files.len(), output.display());
            timing_collector.print();
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
            ui,
        } => {
            let config_file = load_optional_config(&config)?;
            let options = CorpusTestOptions {
                config,
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
                ui,
            };
            run_apis_guru_corpus_test(&config_file, &options)?;
        }
        Command::CorpusSpecWorker {
            spec_path,
            relative_spec,
            config,
            output_directory,
            ignore_unhandled,
            no_go,
            no_python,
            no_typescript,
            output_version,
        } => {
            let config_file = load_optional_config(&config)?;
            let options = CorpusTestOptions {
                config,
                repository: String::new(),
                reference: String::new(),
                checkout_directory: None,
                output_directory,
                report_directory: None,
                ignore_unhandled,
                no_go,
                no_python,
                no_typescript,
                output_version,
                limit: None,
                jobs: None,
                ui: false,
            };
            let result = run_corpus_spec_inline(&config_file, &spec_path, &relative_spec, &options);
            let stdout = std::io::stdout();
            let mut handle = stdout.lock();
            serde_json::to_writer(&mut handle, &result)?;
            handle.write_all(b"\n")?;
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
    timing_collector: &mut TimingCollector,
) -> Result<(CoreIr, Vec<String>)> {
    match (ir, openapi) {
        (Some(ir), None) => timing_collector
            .measure_result("ir_load", || Ok((load_ir(&ir)?, Vec::new()))),
        (None, Some(openapi)) => {
            let emit_timings = timing_collector.enabled;
            let result = timing_collector.measure_result("openapi_load", move || {
                run_with_large_stack("load-openapi", move || {
                    load_openapi_to_ir_with_options(
                        &openapi,
                        openapi_options(ignore_unhandled, emit_timings),
                    )
                })
            })?;
            Ok((result.ir, result.warnings))
        }
        (None, None) => timing_collector.measure_result("ir_load", || {
            Ok((
                load_ir(&PathBuf::from("fixtures/core_ir.json"))?,
                Vec::new(),
            ))
        }),
        (Some(_), Some(_)) => bail!("pass either --ir or --openapi, not both"),
    }
}

fn openapi_options(ignore_unhandled: bool, emit_timings: bool) -> LoadOpenApiOptions {
    LoadOpenApiOptions {
        ignore_unhandled,
        emit_timings,
    }
}

fn print_openapi_warnings(warnings: &[String]) {
    for warning in warnings {
        eprintln!("warning: {warning}");
    }
}

fn format_duration(duration: Duration) -> String {
    if duration.as_secs() >= 1 {
        format!("{:.3}s", duration.as_secs_f64())
    } else if duration.as_millis() >= 1 {
        format!("{}ms", duration.as_millis())
    } else {
        format!("{}us", duration.as_micros())
    }
}

fn run_with_large_stack<T, F>(thread_name: &str, task: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T> + Send + 'static,
{
    let handle = thread::Builder::new()
        .name(thread_name.to_owned())
        .stack_size(OPENAPI_LOAD_STACK_SIZE_BYTES)
        .spawn(task)
        .with_context(|| format!("failed to spawn `{thread_name}` worker"))?;

    match handle.join() {
        Ok(result) => result,
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

struct CorpusTestOptions {
    config: PathBuf,
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
    ui: bool,
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

    let jobs = resolve_corpus_jobs(options.jobs)?;
    eprintln!("running corpus analysis with {jobs} worker(s)...");
    let completed = Arc::new(AtomicUsize::new(0));
    let progress_state = Arc::new(Mutex::new(CorpusProgressState::default()));
    let heartbeat_done = Arc::new(AtomicBool::new(false));
    let total_specs = specs.len();
    let use_ui = options.ui && std::io::stderr().is_terminal();
    let ui_active = Arc::new(AtomicBool::new(use_ui));
    if options.ui && !use_ui {
        eprintln!("`--ui` requested, but stderr is not a terminal; falling back to plain output.");
    }
    let monitor = if use_ui {
        CorpusMonitor::Ui(spawn_corpus_ui(
            Arc::clone(&completed),
            Arc::clone(&progress_state),
            Arc::clone(&heartbeat_done),
            Arc::clone(&ui_active),
            total_specs,
        ))
    } else {
        CorpusMonitor::Heartbeat(spawn_corpus_heartbeat(
            Arc::clone(&completed),
            Arc::clone(&progress_state),
            Arc::clone(&heartbeat_done),
            total_specs,
        ))
    };

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
                    {
                        let mut progress = progress_state
                            .lock()
                            .expect("corpus progress state should not be poisoned");
                        progress.active_specs.insert(relative_spec.clone());
                    }

                    let spec_result = run_corpus_spec_subprocess(
                        config_file,
                        spec_path,
                        &relative_spec,
                        &options.config,
                        options,
                    );

                    let failed = spec_result.failure.is_some()
                        || spec_result
                            .targets
                            .iter()
                            .any(|target| target.failure.is_some());
                    {
                        let mut progress = progress_state
                            .lock()
                            .expect("corpus progress state should not be poisoned");
                        progress.active_specs.remove(&relative_spec);
                        if failed {
                            progress.failed_specs += 1;
                        } else {
                            progress.passed_specs += 1;
                        }
                        progress.recent_completed.push_front(CompletedCorpusSpec {
                            spec: relative_spec.clone(),
                            status: if failed { "failed" } else { "passed" },
                        });
                        while progress.recent_completed.len() > CORPUS_UI_RECENT_LIMIT {
                            progress.recent_completed.pop_back();
                        }
                    }
                    let done = completed.fetch_add(1, Ordering::Relaxed) + 1;
                    let status = if failed { "failed" } else { "passed" };
                    if !ui_active.load(Ordering::Relaxed) {
                        eprintln!("[{done}/{total_specs}] {relative_spec} ({status})");
                    }

                    (index, spec_result)
                })
                .collect::<Vec<_>>()
        });
    heartbeat_done.store(true, Ordering::Relaxed);
    monitor.join();
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
    eprintln!("wrote report to {}", report_path.display());

    eprintln!(
        "completed APIs.guru corpus run: {passed_specs}/{total_specs} specs passed"
    );
    print_failure_summary(&report_data.summary);

    if failed_specs > 0 {
        bail!("{failed_specs} spec(s) failed the APIs.guru corpus run");
    }

    Ok(())
}

fn run_corpus_spec_inline(
    config_file: &AppConfig,
    spec_path: &Path,
    relative_spec: &str,
    options: &CorpusTestOptions,
) -> CorpusSpecResult {
    let go_config = (!options.no_go && !config_file.output.go.disabled).then(|| {
        resolve_go_config(
            config_file,
            None,
            None,
            None,
            false,
            options.output_version.clone(),
        )
    });
    let python_config = (!options.no_python && !config_file.output.python.disabled).then(|| {
        resolve_python_config(
            config_file,
            None,
            None,
            false,
            options.output_version.clone(),
        )
    });
    let typescript_config =
        (!options.no_typescript && !config_file.output.typescript.disabled).then(|| {
            resolve_typescript_config(
                config_file,
                None,
                None,
                false,
                options.output_version.clone(),
            )
        });

    match load_openapi_to_ir_with_options(
        spec_path,
        openapi_options(options.ignore_unhandled, false),
    ) {
        Ok(OpenApiLoadResult { ir, warnings }) => {
            let mut targets = Vec::new();

            if let Some(go_config) = go_config.as_ref() {
                targets.push(run_go_corpus_target(&ir, relative_spec, options, go_config));
            }
            if let Some(python_config) = python_config.as_ref() {
                targets.push(run_python_corpus_target(
                    &ir,
                    relative_spec,
                    options,
                    python_config,
                ));
            }
            if let Some(typescript_config) = typescript_config.as_ref() {
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

fn run_corpus_spec_subprocess(
    config_file: &AppConfig,
    spec_path: &Path,
    relative_spec: &str,
    config_path: &Path,
    options: &CorpusTestOptions,
) -> CorpusSpecResult {
    let current_exe = match std::env::current_exe() {
        Ok(path) => path,
        Err(error) => {
            return CorpusSpecResult {
                spec: relative_spec.to_owned(),
                warning_count: 0,
                targets: Vec::new(),
                failure: Some(classify_failure(
                    &format!(
                        "failed to locate current executable for corpus worker `{relative_spec}`: {error:#}"
                    ),
                    None,
                )),
            };
        }
    };

    let mut command = ProcessCommand::new(current_exe);
    command
        .arg("corpus-spec-worker")
        .arg("--spec-path")
        .arg(spec_path)
        .arg("--relative-spec")
        .arg(relative_spec)
        .arg("--config")
        .arg(config_path);

    if let Some(output_directory) = &options.output_directory {
        command.arg("--output-directory").arg(output_directory);
    }
    if options.ignore_unhandled {
        command.arg("--ignore-unhandled");
    }
    if config_file.output.go.disabled || options.no_go {
        command.arg("--no-go");
    }
    if config_file.output.python.disabled || options.no_python {
        command.arg("--no-python");
    }
    if config_file.output.typescript.disabled || options.no_typescript {
        command.arg("--no-typescript");
    }
    if let Some(output_version) = &options.output_version {
        command.arg("--output-version").arg(output_version);
    }

    let output = match command.output() {
        Ok(output) => output,
        Err(error) => {
            return CorpusSpecResult {
                spec: relative_spec.to_owned(),
                warning_count: 0,
                targets: Vec::new(),
                failure: Some(classify_failure(
                    &format!(
                        "failed to spawn corpus worker for `{relative_spec}`: {error:#}"
                    ),
                    None,
                )),
            };
        }
    };

    if output.status.success() {
        return match serde_json::from_slice::<CorpusSpecResult>(&output.stdout) {
            Ok(result) => result,
            Err(error) => CorpusSpecResult {
                spec: relative_spec.to_owned(),
                warning_count: 0,
                targets: Vec::new(),
                failure: Some(classify_failure(
                    &format!(
                        "failed to parse corpus worker output for `{relative_spec}`: {error:#}\nstdout:\n{}\nstderr:\n{}",
                        String::from_utf8_lossy(&output.stdout),
                        String::from_utf8_lossy(&output.stderr)
                    ),
                    None,
                )),
            },
        };
    }

    let signal = exit_status_signal(&output.status);
    let message = match signal {
        Some(signal) => format!(
            "corpus worker crashed for `{relative_spec}` with signal {signal}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        ),
        None => format!(
            "corpus worker failed for `{relative_spec}` with exit code {:?}\nstderr:\n{}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        ),
    };

    CorpusSpecResult {
        spec: relative_spec.to_owned(),
        warning_count: 0,
        targets: Vec::new(),
        failure: Some(classify_failure(&message, None)),
    }
}

enum CorpusMonitor {
    Heartbeat(thread::JoinHandle<()>),
    Ui(thread::JoinHandle<()>),
}

impl CorpusMonitor {
    fn join(self) {
        match self {
            Self::Heartbeat(handle) | Self::Ui(handle) => {
                let _ = handle.join();
            }
        }
    }
}

fn spawn_corpus_heartbeat(
    completed: Arc<AtomicUsize>,
    progress_state: Arc<Mutex<CorpusProgressState>>,
    done: Arc<AtomicBool>,
    total_specs: usize,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let frames = ["|", "/", "-", "\\"];
        let mut frame_index = 0usize;

        loop {
            thread::sleep(CORPUS_HEARTBEAT_INTERVAL);
            if done.load(Ordering::Relaxed) {
                break;
            }

            let completed_count = completed.load(Ordering::Relaxed);
            let snapshot = {
                let progress = progress_state
                    .lock()
                    .expect("corpus progress state should not be poisoned");
                build_corpus_progress_snapshot(&progress)
            };
            let active_count = snapshot.active_specs.len();
            let sample = snapshot
                .active_specs
                .into_iter()
                .take(3)
                .collect::<Vec<_>>();

            if active_count == 0 || completed_count >= total_specs {
                continue;
            }

            let remaining = total_specs.saturating_sub(completed_count);
            let suffix = if active_count > sample.len() {
                format!(" | ... +{}", active_count - sample.len())
            } else {
                String::new()
            };
            let sample_text = if sample.is_empty() {
                "working...".to_owned()
            } else {
                format!("{}{}", sample.join(" | "), suffix)
            };

            eprintln!(
                "[heartbeat {}] active {} worker(s), completed {}/{} (remaining {}): {}",
                frames[frame_index % frames.len()],
                active_count,
                completed_count,
                total_specs,
                remaining,
                sample_text
            );
            frame_index += 1;
        }
    })
}

fn spawn_corpus_ui(
    completed: Arc<AtomicUsize>,
    progress_state: Arc<Mutex<CorpusProgressState>>,
    done: Arc<AtomicBool>,
    ui_active: Arc<AtomicBool>,
    total_specs: usize,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        if let Err(error) = run_corpus_ui(completed, progress_state, done, ui_active, total_specs)
        {
            eprintln!("failed to render corpus UI: {error:#}");
        }
    })
}

fn run_corpus_ui(
    completed: Arc<AtomicUsize>,
    progress_state: Arc<Mutex<CorpusProgressState>>,
    done: Arc<AtomicBool>,
    ui_active: Arc<AtomicBool>,
    total_specs: usize,
) -> Result<()> {
    enable_raw_mode().context("failed to enable raw mode for corpus UI")?;
    let mut stdout = std::io::stderr();
    execute!(stdout, EnterAlternateScreen).context("failed to enter alternate screen")?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).context("failed to initialize corpus UI")?;
    let spinner_frames = ["|", "/", "-", "\\"];
    let mut spinner_index = 0usize;

    loop {
        let completed_count = completed.load(Ordering::Relaxed);
        let snapshot = {
            let progress = progress_state
                .lock()
                .expect("corpus progress state should not be poisoned");
            build_corpus_progress_snapshot(&progress)
        };

        terminal
            .draw(|frame| {
                let area = frame.area();
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(3),
                        Constraint::Length(7),
                        Constraint::Min(8),
                        Constraint::Length(6),
                    ])
                    .split(area);

                let progress_ratio = if total_specs == 0 {
                    0.0
                } else {
                    completed_count as f64 / total_specs as f64
                };
                let progress_label = format!(
                    "{} {}/{} ({:.1}%)",
                    spinner_frames[spinner_index % spinner_frames.len()],
                    completed_count,
                    total_specs,
                    progress_ratio * 100.0
                );
                let gauge = Gauge::default()
                    .block(Block::default().title("Corpus Progress").borders(Borders::ALL))
                    .gauge_style(
                        Style::default()
                            .fg(Color::Cyan)
                            .bg(Color::Black)
                            .add_modifier(Modifier::BOLD),
                    )
                    .label(progress_label)
                    .ratio(progress_ratio);
                frame.render_widget(gauge, chunks[0]);

                let active_count = snapshot.active_specs.len();
                let remaining = total_specs.saturating_sub(completed_count);
                let stats = Paragraph::new(vec![
                    Line::from(vec![
                        Span::styled(
                            format!("Passed: {}", snapshot.passed_specs),
                            Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
                        ),
                        Span::raw("    "),
                        Span::styled(
                            format!("Failed: {}", snapshot.failed_specs),
                            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                        ),
                    ]),
                    Line::from(format!("Active workers: {active_count}")),
                    Line::from(format!("Remaining specs: {remaining}")),
                    Line::from("Press q to hide the UI and continue with plain progress."),
                ])
                .block(Block::default().title("Run Stats").borders(Borders::ALL));
                frame.render_widget(stats, chunks[1]);

                let middle = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
                    .split(chunks[2]);

                let active_items = if snapshot.active_specs.is_empty() {
                    vec![ListItem::new("No active specs right now")]
                } else {
                    snapshot
                        .active_specs
                        .iter()
                        .take(CORPUS_UI_ACTIVE_SAMPLE_LIMIT)
                        .map(|spec| ListItem::new(spec.clone()))
                        .collect::<Vec<_>>()
                };
                let active_list = List::new(active_items)
                    .block(Block::default().title("Active Specs").borders(Borders::ALL));
                frame.render_widget(active_list, middle[0]);

                let recent_items = if snapshot.recent_completed.is_empty() {
                    vec![ListItem::new("No completed specs yet")]
                } else {
                    snapshot
                        .recent_completed
                        .iter()
                        .map(|entry| {
                            let style = if entry.status == "passed" {
                                Style::default().fg(Color::Green)
                            } else {
                                Style::default().fg(Color::Red)
                            };
                            ListItem::new(Line::from(vec![
                                Span::styled(format!("[{}] ", entry.status), style),
                                Span::raw(entry.spec.clone()),
                            ]))
                        })
                        .collect::<Vec<_>>()
                };
                let recent_list = List::new(recent_items).block(
                    Block::default()
                        .title("Recent Completions")
                        .borders(Borders::ALL),
                );
                frame.render_widget(recent_list, middle[1]);

                let footer = Paragraph::new(vec![
                    Line::from("The corpus run continues even if one spec crashes."),
                    Line::from("Close the UI with q if you want the plain line-based progress instead."),
                ])
                .block(Block::default().title("Notes").borders(Borders::ALL));
                frame.render_widget(footer, chunks[3]);
            })
            .context("failed to draw corpus UI")?;

        spinner_index += 1;

        if done.load(Ordering::Relaxed) {
            break;
        }

        if event::poll(CORPUS_UI_TICK_INTERVAL).context("failed while polling corpus UI")? {
            if let Event::Key(key) = event::read().context("failed while reading corpus UI input")?
            {
                if key.code == KeyCode::Char('q') {
                    ui_active.store(false, Ordering::Relaxed);
                    break;
                }
            }
        }
    }

    disable_raw_mode().context("failed to disable raw mode for corpus UI")?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)
        .context("failed to leave alternate screen")?;
    terminal.show_cursor().context("failed to restore cursor")?;
    Ok(())
}

fn build_corpus_progress_snapshot(progress: &CorpusProgressState) -> CorpusProgressSnapshot {
    CorpusProgressSnapshot {
        active_specs: progress.active_specs.iter().cloned().collect(),
        recent_completed: progress.recent_completed.iter().cloned().collect(),
        passed_specs: progress.passed_specs,
        failed_specs: progress.failed_specs,
    }
}

#[cfg(unix)]
fn exit_status_signal(status: &ExitStatus) -> Option<i32> {
    status.signal()
}

#[cfg(not(unix))]
fn exit_status_signal(_status: &ExitStatus) -> Option<i32> {
    None
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
    let (line, column) = extract_line_and_column(message);
    let source_preview = extract_source_preview(message);
    let note = extract_note(message);

    let make_failure = |kind: String, feature: String| CorpusFailure {
        kind,
        feature,
        pointer: pointer.clone(),
        schema_path: schema_path.map(str::to_owned),
        line,
        column,
        source_preview: source_preview.clone(),
        note: note.clone(),
        target: target.map(str::to_owned),
        message: message.to_owned(),
    };

    if let Some(keyword) = extract_between(message, "unknown schema keyword `", "`") {
        return make_failure("unsupported_schema_keyword".into(), keyword.to_owned());
    }

    if let Some(schema_type) = extract_between(message, "unsupported schema type `", "`") {
        return make_failure("unsupported_schema_type".into(), schema_type.to_owned());
    }

    if let Some(reference) = extract_between(message, "unsupported reference `", "`") {
        return make_failure("unsupported_reference".into(), reference.to_owned());
    }

    if message.contains("request body has no content entries") {
        return make_failure(
            "unsupported_request_body_shape".into(),
            "empty_content".into(),
        );
    }

    if let Some(field) = extract_between(message, "`allOf` contains incompatible `", "` declarations")
    {
        return make_failure("unsupported_all_of_merge".into(), field.to_owned());
    }

    if let Some(feature) = extract_between(message, "`", "` is not supported yet") {
        return make_failure(
            classify_not_supported_kind(pointer.as_deref(), feature),
            feature.to_owned(),
        );
    }

    if message.contains("schema shape is not supported yet") {
        return make_failure(
            "unsupported_schema_shape".into(),
            pointer
                .as_deref()
                .map(pointer_tail_feature)
                .or_else(|| schema_path.map(normalize_feature))
                .unwrap_or_else(|| "schema_shape".into()),
        );
    }

    if message.contains("failed to parse JSON OpenAPI document")
        || message.contains("failed to parse YAML OpenAPI document")
    {
        return make_failure(
            "invalid_openapi_document".into(),
            schema_path
                .map(normalize_feature)
                .unwrap_or_else(|| "deserialization".into()),
        );
    }

    if message.contains("generated IR is invalid") {
        return make_failure("ir_validation_error".into(), "invalid_ir".into());
    }

    if message.contains("corpus worker crashed")
        || message.contains("stack overflow")
        || message.contains("terminated by SIGABRT")
    {
        return make_failure("process_crash".into(), "stack_overflow".into());
    }

    if let Some(target_name) = target {
        if message.contains("failed to write generated files") {
            return make_failure(
                "target_write_error".into(),
                format!("{target_name}.write_output"),
            );
        }

        if message.contains("failed to generate") {
            return make_failure(
                "target_generation_error".into(),
                format!("{target_name}.generation"),
            );
        }
    }

    if message.contains("parameter has no schema or type")
        || message.contains("formData parameter has no schema or type")
    {
        let param_name = extract_between(message, "parameter `", "`:")
            .map(normalize_feature)
            .unwrap_or_else(|| "parameter_missing_schema".into());
        return make_failure("invalid_openapi_document".into(), param_name);
    }

    if message.contains("has an empty name") {
        return make_failure(
            "invalid_openapi_document".into(),
            pointer
                .as_deref()
                .map(pointer_tail_feature)
                .unwrap_or_else(|| "empty_name".into()),
        );
    }

    if message.contains("array schema is missing `items`") {
        return make_failure(
            "invalid_openapi_document".into(),
            pointer
                .as_deref()
                .map(pointer_tail_feature)
                .unwrap_or_else(|| "missing_array_items".into()),
        );
    }

    make_failure("unknown_error".into(), "unknown".into())
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

fn extract_line_and_column(message: &str) -> (Option<usize>, Option<usize>) {
    for line in message.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("location: line ") {
            let (line_number, column_part) = match rest.split_once(", column ") {
                Some(values) => values,
                None => continue,
            };
            let line_value = line_number.trim().parse::<usize>().ok();
            let column_value = column_part.trim().parse::<usize>().ok();
            return (line_value, column_value);
        }
    }

    (None, None)
}

fn extract_source_preview(message: &str) -> Option<String> {
    if let Some(preview) = extract_indented_block(message, "preview:") {
        return Some(preview);
    }

    let lines = message.lines().collect::<Vec<_>>();
    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if let Some(source_line) = trimmed.strip_prefix("source: ") {
            let mut preview = source_line.to_owned();
            if let Some(next_line) = lines.get(index + 1) {
                let next_trimmed = next_line.trim_start();
                if next_trimmed.starts_with('^') {
                    preview.push('\n');
                    preview.push_str(next_trimmed);
                }
            }
            return Some(preview);
        }
    }

    None
}

fn extract_note(message: &str) -> Option<String> {
    for line in message.lines() {
        let trimmed = line.trim_start();
        if let Some(note) = trimmed.strip_prefix("note: ") {
            return Some(note.to_owned());
        }
    }
    None
}

fn extract_indented_block(message: &str, label: &str) -> Option<String> {
    let lines = message.lines().collect::<Vec<_>>();
    let start_index = lines
        .iter()
        .position(|line| line.trim_start() == label)?;

    let mut block = Vec::new();
    for line in lines.into_iter().skip(start_index + 1) {
        if let Some(rest) = line.strip_prefix("    ") {
            block.push(rest);
            continue;
        }
        if line.trim().is_empty() && !block.is_empty() {
            block.push("");
            continue;
        }
        break;
    }

    if block.is_empty() {
        None
    } else {
        Some(block.join("\n"))
    }
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

#[cfg(test)]
mod tests {
    use super::classify_failure;

    #[test]
    fn classifies_empty_request_body_content() {
        let failure = classify_failure(
            "OpenAPI document issue\nCaused by:\n  request body has no content entries\n  location: #/paths/~1widgets/post/requestBody/content\n  note: Arvalez expects at least one media type under `requestBody.content`.",
            None,
        );
        assert_eq!(failure.kind, "unsupported_request_body_shape");
        assert_eq!(failure.feature, "empty_content");
        assert_eq!(
            failure.pointer.as_deref(),
            Some("#/paths/~1widgets/post/requestBody/content")
        );
    }

    #[test]
    fn classifies_incompatible_all_of_declarations() {
        let failure = classify_failure(
            "OpenAPI document issue\nCaused by:\n  `allOf` contains incompatible `title` declarations\n  location: #/components/schemas/Foo\n  preview:\n    allOf:\n    - $ref: '#/components/schemas/Bar'\n  note: Use `--ignore-unhandled` to turn this into a warning while keeping generation going.",
            None,
        );
        assert_eq!(failure.kind, "unsupported_all_of_merge");
        assert_eq!(failure.feature, "title");
        assert_eq!(failure.pointer.as_deref(), Some("#/components/schemas/Foo"));
    }

    #[test]
    fn classifies_parameter_missing_schema() {
        let failure = classify_failure(
            "parameter `x-apideck-metadata`: parameter has no schema or type\nnote: Arvalez currently expects non-body parameters to declare either `schema` (OpenAPI 3) or `type` (Swagger 2).",
            None,
        );
        assert_eq!(failure.kind, "invalid_openapi_document");
        assert_eq!(failure.feature, "x-apideck-metadata");
    }

    #[test]
    fn classifies_parameter_with_empty_name() {
        let failure = classify_failure(
            "operation `customers`: parameter #1 has an empty name\nnote: Use `--ignore-unhandled` to turn this into a warning while keeping generation going.",
            None,
        );
        assert_eq!(failure.kind, "invalid_openapi_document");
        assert_eq!(failure.feature, "empty_name");
    }

    #[test]
    fn classifies_property_with_empty_name() {
        let failure = classify_failure(
            "OpenAPI document issue\nCaused by:\n  property #1 has an empty name\n  location: #/components/schemas/shared-user/properties\n  preview:\n    '':\n      type: string\n    username:\n      type: string\n  note: Use `--ignore-unhandled` to turn this into a warning while keeping generation going.",
            None,
        );
        assert_eq!(failure.kind, "invalid_openapi_document");
        assert_eq!(failure.feature, "properties");
        assert_eq!(
            failure.pointer.as_deref(),
            Some("#/components/schemas/shared-user/properties")
        );
    }

    #[test]
    fn classifies_array_missing_items() {
        let failure = classify_failure(
            "OpenAPI document issue\nCaused by:\n  array schema is missing `items`\n  location: #/definitions/DBResp/properties/Data\n  preview:\n    example:\n    - <array of data objects>\n    type: array\n  note: Add an `items` schema to describe the array element type.",
            None,
        );
        assert_eq!(failure.kind, "invalid_openapi_document");
        assert_eq!(failure.feature, "Data");
        assert_eq!(
            failure.pointer.as_deref(),
            Some("#/definitions/DBResp/properties/Data")
        );
    }
}
