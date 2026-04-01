mod config;
mod corpus;
mod corpus_ui;
mod generate;
#[cfg(test)]
mod tests;

use std::{io::Write, path::PathBuf};

use anyhow::{Result, bail};
use arvalez_openapi::{OpenApiLoadResult, load_openapi_to_ir_with_options};
use arvalez_target_core::CommonConfig;
use arvalez_target_go::{generate_go_package, write_go_package};
use arvalez_target_nushell::{generate_nushell_package, write_nushell_package};
use arvalez_target_python::{generate_python_package, write_python_package};
use arvalez_target_pythonmini::{
    TargetConfig, generate as generate_pythonmini_package,
    write_package as write_pythonmini_package,
};
use arvalez_target_pythonmini::{
    dump_erasers as dump_pythonmini_erasers, dump_templates as dump_pythonmini_templates,
};
use arvalez_target_typescript::{generate_typescript_package, write_typescript_package};
use clap::{Parser, Subcommand, ValueEnum};
use config::{
    load_optional_config, resolve_go_config, resolve_nushell_config, resolve_output_root,
    resolve_python_config, resolve_target_output_directory, resolve_typescript_config,
};
use corpus::{CorpusTestOptions, run_apis_guru_corpus_test, run_corpus_spec_inline};
use generate::{
    TimingCollector, load_input_ir, load_ir, openapi_options, print_openapi_warnings,
    run_with_large_stack,
};

/// A target to dump templates or erasers for.
#[derive(Clone, ValueEnum)]
enum DumpTarget {
    PythonMini,
}

impl DumpTarget {
    fn slug(&self) -> &'static str {
        match self {
            DumpTarget::PythonMini => "python-mini",
        }
    }
}

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
        no_nushell: bool,
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
    GenerateNushell {
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
        module_name: Option<String>,
        #[arg(long)]
        template_dir: Option<PathBuf>,
        #[arg(long)]
        default_base_url: Option<String>,
        #[arg(long)]
        group_by_tag: bool,
        #[arg(long)]
        output_version: Option<String>,
        #[arg(long)]
        timings: bool,
    },
    GeneratePythonMini {
        #[arg(long)]
        ir: Option<PathBuf>,
        #[arg(long)]
        openapi: Option<PathBuf>,
        #[arg(long = "output-directory")]
        output_directory: Option<PathBuf>,
        #[arg(long)]
        ignore_unhandled: bool,
        #[arg(long)]
        package_name: Option<String>,
        #[arg(long)]
        version: Option<String>,
        #[arg(long)]
        template_dir: Option<PathBuf>,
        #[arg(long)]
        timings: bool,
    },
    /// Dump the built-in templates for a target to a directory so they can be
    /// inspected and customised. Pass the directory to `--template-dir` to use
    /// your overrides.
    DumpTemplates {
        #[arg(long)]
        target: DumpTarget,
        /// Destination directory (defaults to `templates/<target>`).
        #[arg(long = "output-directory")]
        output_directory: Option<PathBuf>,
    },
    /// Dump empty tilde-prefixed eraser files for a target. Placing any of
    /// these in your `--template-dir` suppresses generation of the
    /// corresponding output file entirely.
    DumpErasers {
        #[arg(long)]
        target: DumpTarget,
        /// Destination directory (defaults to `templates/<target>`).
        #[arg(long = "output-directory")]
        output_directory: Option<PathBuf>,
    },
    TestApisGuru {
        #[arg(
            long,
            default_value = "https://github.com/APIs-guru/openapi-directory.git"
        )]
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
        no_nushell: bool,
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
        no_nushell: bool,
        #[arg(long)]
        output_version: Option<String>,
    },
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
            let rendered_ir = timing_collector
                .measure_result("ir_serialize", || Ok(serde_json::to_string_pretty(&ir)?))?;
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
            no_nushell,
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
            let nushell_enabled = !no_nushell && !config_file.output.nushell.disabled;

            if !go_enabled && !python_enabled && !typescript_enabled && !nushell_enabled {
                bail!("no generation targets enabled");
            }

            if go_enabled {
                let go_config = resolve_go_config(
                    &config_file,
                    None,
                    None,
                    None,
                    false,
                    output_version.clone(),
                );
                let files = timing_collector
                    .measure_result("go_generate", || generate_go_package(&ir, &go_config))?;
                let output = output_root.join("go-client");
                timing_collector
                    .measure_result("go_write", || write_go_package(&output, &files))?;
                eprintln!("generated {} files into {}", files.len(), output.display());
            }

            if python_enabled {
                let python_config =
                    resolve_python_config(&config_file, None, None, false, output_version.clone());
                let files = timing_collector.measure_result("python_generate", || {
                    generate_python_package(&ir, &python_config)
                })?;
                let output = output_root.join("python-client");
                timing_collector
                    .measure_result("python_write", || write_python_package(&output, &files))?;
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

            if nushell_enabled {
                let nushell_config = resolve_nushell_config(
                    &config_file,
                    None,
                    None,
                    None,
                    false,
                    output_version.clone(),
                );
                let files = timing_collector.measure_result("nushell_generate", || {
                    generate_nushell_package(&ir, &nushell_config)
                })?;
                let output = output_root.join("nushell-client");
                timing_collector
                    .measure_result("nushell_write", || write_nushell_package(&output, &files))?;
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
            let output =
                resolve_target_output_directory(&config_file, output_directory, "go-client");
            let go_config = resolve_go_config(
                &config_file,
                module_path,
                package_name,
                template_dir,
                group_by_tag,
                output_version,
            );
            let files = timing_collector
                .measure_result("go_generate", || generate_go_package(&ir, &go_config))?;
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
            timing_collector
                .measure_result("python_write", || write_python_package(&output, &files))?;
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
        Command::GenerateNushell {
            ir,
            openapi,
            config,
            output_directory,
            ignore_unhandled,
            module_name,
            template_dir,
            default_base_url,
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
                resolve_target_output_directory(&config_file, output_directory, "nushell-client");
            let nushell_config = resolve_nushell_config(
                &config_file,
                module_name,
                template_dir,
                default_base_url,
                group_by_tag,
                output_version,
            );
            let files = timing_collector.measure_result("nushell_generate", || {
                generate_nushell_package(&ir, &nushell_config)
            })?;
            timing_collector
                .measure_result("nushell_write", || write_nushell_package(&output, &files))?;
            eprintln!("generated {} files into {}", files.len(), output.display());
            timing_collector.print();
        }
        Command::GeneratePythonMini {
            ir,
            openapi,
            output_directory,
            ignore_unhandled,
            package_name,
            version,
            template_dir,
            timings,
        } => {
            let mut timing_collector = TimingCollector::new(timings);
            let (ir, warnings) =
                load_input_ir(ir, openapi, ignore_unhandled, &mut timing_collector)?;
            print_openapi_warnings(&warnings);
            let output = output_directory.unwrap_or_else(|| PathBuf::from("pythonmini-client"));
            let common = CommonConfig {
                package_name: package_name.unwrap_or_else(|| "client".into()),
                version: version.unwrap_or_else(|| "0.1.0".into()),
            };
            let config = TargetConfig {};
            let files = timing_collector.measure_result("pythonmini_generate", || {
                generate_pythonmini_package(&ir, template_dir.as_deref(), &common, &config)
            })?;
            timing_collector.measure_result("pythonmini_write", || {
                write_pythonmini_package(&output, &files)
            })?;
            eprintln!("generated {} files into {}", files.len(), output.display());
            timing_collector.print();
        }
        Command::DumpTemplates {
            target,
            output_directory,
        } => {
            let dir = output_directory
                .unwrap_or_else(|| PathBuf::from(format!("templates/{}", target.slug())));
            match target {
                DumpTarget::PythonMini => dump_pythonmini_templates(&dir)?,
            }
            eprintln!("dumped templates into {}", dir.display());
        }
        Command::DumpErasers {
            target,
            output_directory,
        } => {
            let dir = output_directory
                .unwrap_or_else(|| PathBuf::from(format!("templates/{}", target.slug())));
            match target {
                DumpTarget::PythonMini => dump_pythonmini_erasers(&dir)?,
            }
            eprintln!("dumped erasers into {}", dir.display());
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
            no_nushell,
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
                no_nushell,
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
            no_nushell,
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
                no_nushell,
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
