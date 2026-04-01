use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fs,
    io::IsTerminal,
    path::{Path, PathBuf},
    process::{Command as ProcessCommand, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result, bail};
use arvalez_ir::CoreIr;
use arvalez_openapi::{
    OpenApiDiagnostic, OpenApiLoadResult, categorize_reference,
    diagnostic_pointer_tail, load_openapi_to_ir_with_options, normalize_diagnostic_feature,
};
use arvalez_target_go::{generate_go_package, write_go_package};
use arvalez_target_python::{PythonPackageConfig, generate_python_package, write_python_package};
use arvalez_target_typescript::{
    TypeScriptPackageConfig, generate_typescript_package, write_typescript_package,
};
use arvalez_target_nushell::{NushellPackageConfig, generate_nushell_package, write_nushell_package};
use serde::{Deserialize, Serialize};
use tempfile::TempDir;

use crate::config::AppConfig;
use crate::corpus_ui::{
    CorpusMonitor, exit_status_signal, spawn_corpus_heartbeat,
    spawn_corpus_ui,
};
use crate::generate::openapi_options;

pub(crate) const CORPUS_WORKER_STACK_SIZE_BYTES: usize = 16 * 1024 * 1024;
pub(crate) const CORPUS_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(3);
pub(crate) const CORPUS_UI_RECENT_LIMIT: usize = 8;

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct CorpusReport {
    pub generated_at_unix_seconds: u64,
    pub repository: String,
    pub reference: String,
    pub total_specs: usize,
    pub passed_specs: usize,
    pub failed_specs: usize,
    pub summary: CorpusFailureSummary,
    pub results: Vec<CorpusSpecResult>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct CorpusSpecResult {
    pub spec: String,
    pub warning_count: usize,
    pub targets: Vec<CorpusTargetResult>,
    pub failure: Option<CorpusFailure>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct CorpusTargetResult {
    pub name: String,
    pub generated_files: usize,
    pub failure: Option<CorpusFailure>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct CorpusFailure {
    pub kind: String,
    pub feature: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pointer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_preview: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct CorpusFailureSummary {
    pub total_failures: usize,
    pub by_kind: BTreeMap<String, usize>,
    pub by_kind_and_feature: BTreeMap<String, usize>,
}

#[derive(Debug, Clone)]
pub(crate) struct CompletedCorpusSpec {
    pub spec: String,
    pub status: &'static str,
}

#[derive(Debug, Default)]
pub(crate) struct CorpusProgressSnapshot {
    pub active_specs: Vec<String>,
    pub recent_completed: Vec<CompletedCorpusSpec>,
    pub passed_specs: usize,
    pub failed_specs: usize,
}

#[derive(Debug, Default)]
pub(crate) struct CorpusProgressState {
    pub active_specs: BTreeSet<String>,
    pub recent_completed: VecDeque<CompletedCorpusSpec>,
    pub passed_specs: usize,
    pub failed_specs: usize,
}

pub(crate) struct CorpusTestOptions {
    pub config: PathBuf,
    pub repository: String,
    pub reference: String,
    pub checkout_directory: Option<PathBuf>,
    pub output_directory: Option<PathBuf>,
    pub report_directory: Option<PathBuf>,
    pub ignore_unhandled: bool,
    pub no_go: bool,
    pub no_python: bool,
    pub no_typescript: bool,
    pub no_nushell: bool,
    pub output_version: Option<String>,
    pub limit: Option<usize>,
    pub jobs: Option<usize>,
    pub ui: bool,
}

pub(crate) fn run_apis_guru_corpus_test(
    config_file: &AppConfig,
    options: &CorpusTestOptions,
) -> Result<()> {
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
    let nushell_enabled = !options.no_nushell && !config_file.output.nushell.disabled;

    if !go_enabled && !python_enabled && !typescript_enabled && !nushell_enabled {
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

    use rayon::{ThreadPoolBuilder, prelude::*};

    let mut indexed_results = ThreadPoolBuilder::new()
        .num_threads(jobs)
        .stack_size(CORPUS_WORKER_STACK_SIZE_BYTES)
        .build()
        .context("failed to build corpus worker pool")?
        .install(|| {
            specs
                .par_iter()
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
                || result.targets.iter().any(|target| target.failure.is_some())
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

pub(crate) fn run_corpus_spec_inline(
    config_file: &AppConfig,
    spec_path: &Path,
    relative_spec: &str,
    options: &CorpusTestOptions,
) -> CorpusSpecResult {
    use crate::config::{resolve_go_config, resolve_nushell_config, resolve_python_config, resolve_typescript_config};

    let go_config = (!options.no_go && !config_file.output.go.disabled).then(|| {
        resolve_go_config(config_file, None, None, None, false, options.output_version.clone())
    });
    let python_config = (!options.no_python && !config_file.output.python.disabled).then(|| {
        resolve_python_config(config_file, None, None, false, options.output_version.clone())
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
    let nushell_config =
        (!options.no_nushell && !config_file.output.nushell.disabled).then(|| {
            resolve_nushell_config(config_file, None, None, None, false, options.output_version.clone())
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
            if let Some(nushell_config) = nushell_config.as_ref() {
                targets.push(run_nushell_corpus_target(
                    &ir,
                    relative_spec,
                    options,
                    nushell_config,
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
            failure: Some(
                error
                    .downcast_ref::<OpenApiDiagnostic>()
                    .map(|diag| corpus_failure_from_diagnostic(diag, None))
                    .unwrap_or_else(|| classify_failure(&format!("{error:#}"), None)),
            ),
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
    if config_file.output.nushell.disabled || options.no_nushell {
        command.arg("--no-nushell");
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
                    &format!("failed to spawn corpus worker for `{relative_spec}`: {error:#}"),
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

fn run_go_corpus_target(
    ir: &CoreIr,
    relative_spec: &str,
    options: &CorpusTestOptions,
    config: &(arvalez_target_core::CommonConfig, arvalez_target_go::TargetConfig, Option<std::path::PathBuf>),
) -> CorpusTargetResult {
    match generate_go_package(ir, config.2.as_deref(), &config.0, &config.1) {
        Ok(files) => write_corpus_target_output(
            relative_spec,
            options,
            "go-client",
            &files,
            |output, files| write_go_package(output, files),
        ),
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

fn run_nushell_corpus_target(
    ir: &CoreIr,
    relative_spec: &str,
    options: &CorpusTestOptions,
    config: &NushellPackageConfig,
) -> CorpusTargetResult {
    match generate_nushell_package(ir, config) {
        Ok(files) => write_corpus_target_output(
            relative_spec,
            options,
            "nushell-client",
            &files,
            |output, files| write_nushell_package(output, files),
        ),
        Err(error) => CorpusTargetResult {
            name: "nushell".into(),
            generated_files: 0,
            failure: Some(classify_failure(
                &format!("failed to generate nushell client for `{relative_spec}`: {error:#}"),
                Some("nushell"),
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

pub(crate) fn clone_repository(
    repository: &str,
    reference: &str,
    checkout_directory: &Path,
) -> Result<()> {
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
        .args([
            "clone",
            "--depth",
            "1",
            "--branch",
            reference,
            "--single-branch",
        ])
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

fn update_repository_checkout(
    repository: &str,
    reference: &str,
    checkout_directory: &Path,
) -> Result<()> {
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

pub(crate) fn prepare_repository_checkout(
    repository: &str,
    reference: &str,
    checkout_directory: &Path,
) -> Result<()> {
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

pub(crate) fn collect_openapi_json_files(root: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(root)
        .with_context(|| format!("failed to read directory `{}`", root.display()))?
    {
        let entry = entry
            .with_context(|| format!("failed to read entry from directory `{}`", root.display()))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .with_context(|| format!("failed to read file type for `{}`", path.display()))?;

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

pub(crate) fn current_unix_timestamp_seconds() -> Result<u64> {
    use std::time::{SystemTime, UNIX_EPOCH};
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before the unix epoch")?
        .as_secs())
}

pub(crate) fn resolve_corpus_jobs(requested_jobs: Option<usize>) -> Result<usize> {
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

pub(crate) fn write_corpus_report(
    report_directory: &Path,
    report: &CorpusReport,
) -> Result<PathBuf> {
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

pub(crate) fn summarize_failures(results: &[CorpusSpecResult]) -> CorpusFailureSummary {
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

pub(crate) fn print_failure_summary(summary: &CorpusFailureSummary) {
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

/// Convert a typed [`OpenApiDiagnostic`] directly to a [`CorpusFailure`] without
/// string parsing.
pub(crate) fn corpus_failure_from_diagnostic(
    diag: &OpenApiDiagnostic,
    target: Option<&str>,
) -> CorpusFailure {
    let (kind, feature) = diag.classify();
    CorpusFailure {
        kind: kind.into(),
        feature,
        pointer: diag.pointer.clone(),
        schema_path: None,
        line: diag.line,
        column: None,
        source_preview: diag.source_preview.clone(),
        note: diag.note().map(str::to_owned),
        target: target.map(str::to_owned),
        message: diag.to_string(),
    }
}

pub(crate) fn classify_failure(message: &str, target: Option<&str>) -> CorpusFailure {
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
        return make_failure("unsupported_reference".into(), categorize_reference(reference));
    }

    if message.contains("request body has no content entries") {
        return make_failure(
            "unsupported_request_body_shape".into(),
            "empty_content".into(),
        );
    }

    if let Some(field) =
        extract_between(message, "`allOf` contains incompatible `", "` declarations")
    {
        return make_failure("unsupported_all_of_merge".into(), field.to_owned());
    }

    if let Some(feature) = extract_between(message, "`", "` is not supported yet") {
        return make_failure(
            OpenApiDiagnostic::unsupported_kind_for_pointer(pointer.as_deref(), feature).to_owned(),
            feature.to_owned(),
        );
    }

    if message.contains("schema shape is not supported yet") {
        return make_failure(
            "unsupported_schema_shape".into(),
            pointer
                .as_deref()
                .map(diagnostic_pointer_tail)
                .or_else(|| schema_path.map(normalize_diagnostic_feature))
                .unwrap_or_else(|| "schema_shape".into()),
        );
    }

    if message.contains("failed to parse JSON OpenAPI document")
        || message.contains("failed to parse YAML OpenAPI document")
    {
        let feature = if message.contains("JSON number out of range") {
            "number_out_of_range".into()
        } else if message.contains("control characters are not allowed") {
            "control_characters".into()
        } else if message.contains("invalid type:") {
            let expected = extract_between(message, "expected ", " at line")
                .unwrap_or("unknown");
            let stripped = expected
                .strip_prefix("a ")
                .or_else(|| expected.strip_prefix("an "))
                .unwrap_or(expected);
            let normalised = stripped
                .replace(' ', "_")
                .replace('.', "_")
                .replace('`', "")
                .to_lowercase();
            format!("invalid_type_expected_{normalised}")
        } else {
            schema_path
                .and_then(|p| p.rsplit('.').next())
                .map(normalize_diagnostic_feature)
                .unwrap_or_else(|| "deserialization".into())
        };
        return make_failure("invalid_openapi_document".into(), feature);
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
            .map(normalize_diagnostic_feature)
            .unwrap_or_else(|| "parameter_missing_schema".into());
        return make_failure("invalid_openapi_document".into(), param_name);
    }

    if message.contains("parameter #") && message.contains("has an empty name") {
        return make_failure(
            "invalid_openapi_document".into(),
            "empty_parameter_name".into(),
        );
    }

    if message.contains("property #") && message.contains("has an empty name") {
        return make_failure(
            "invalid_openapi_document".into(),
            "empty_property_key".into(),
        );
    }

    if message.contains("array schema is missing `items`") {
        return make_failure(
            "invalid_openapi_document".into(),
            pointer
                .as_deref()
                .map(diagnostic_pointer_tail)
                .unwrap_or_else(|| "missing_array_items".into()),
        );
    }

    make_failure("unknown_error".into(), "unknown".into())
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
    let start_index = lines.iter().position(|line| line.trim_start() == label)?;

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

pub(crate) fn extract_between<'a>(
    message: &'a str,
    prefix: &str,
    suffix: &str,
) -> Option<&'a str> {
    let start = message.find(prefix)? + prefix.len();
    let rest = &message[start..];
    let end = rest.find(suffix)?;
    Some(&rest[..end])
}
