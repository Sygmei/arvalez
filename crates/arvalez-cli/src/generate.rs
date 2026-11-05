use std::{
    fs,
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use arvalez_ir::{CoreIr, validate_ir};
use arvalez_openapi::{
    LoadOpenApiOptions, OpenApiDiagnostic, load_openapi_to_ir_with_options,
};

// Some real-world Azure/Codat specs still recurse deeply enough during import
// that the default Rust thread stack and our earlier 64 MiB budget were not
// sufficient. Keep this comfortably high so corpus runs record real importer
// errors instead of aborting on stack overflow.
pub(crate) const OPENAPI_LOAD_STACK_SIZE_BYTES: usize = 256 * 1024 * 1024;

pub(crate) struct TimingCollector {
    pub(crate) enabled: bool,
    started_at: Instant,
    phases: Vec<(String, Duration)>,
}

impl TimingCollector {
    pub(crate) fn new(enabled: bool) -> Self {
        Self {
            enabled,
            started_at: Instant::now(),
            phases: Vec::new(),
        }
    }

    pub(crate) fn measure_result<T, F>(
        &mut self,
        label: impl Into<String>,
        task: F,
    ) -> Result<T>
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

    pub(crate) fn print(&self) {
        if !self.enabled {
            return;
        }

        eprintln!("timings:");
        for (label, duration) in &self.phases {
            eprintln!("  {:<20} {}", label, format_duration(*duration));
        }
        eprintln!(
            "  {:<20} {}",
            "total",
            format_duration(self.started_at.elapsed())
        );
    }
}

pub(crate) fn load_ir(path: &PathBuf) -> Result<CoreIr> {
    let raw = fs::read(path)
        .with_context(|| format!("failed to read IR fixture `{}`", path.display()))?;
    let ir: CoreIr = serde_json::from_slice(&raw)
        .with_context(|| format!("failed to parse `{}` as CoreIr", path.display()))?;

    if let Err(errors) = validate_ir(&ir) {
        for issue in errors.0 {
            eprintln!("validation error: {issue}");
        }
        bail!("IR fixture is invalid");
    }

    Ok(ir)
}

pub(crate) fn load_input_ir(
    ir: Option<PathBuf>,
    openapi: Option<PathBuf>,
    ignore_unhandled: bool,
    timing_collector: &mut TimingCollector,
) -> Result<(CoreIr, Vec<OpenApiDiagnostic>)> {
    match (ir, openapi) {
        (Some(ir), None) => {
            timing_collector.measure_result("ir_load", || Ok((load_ir(&ir)?, Vec::new())))
        }
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

pub(crate) fn openapi_options(ignore_unhandled: bool, emit_timings: bool) -> LoadOpenApiOptions {
    LoadOpenApiOptions {
        ignore_unhandled,
        emit_timings,
    }
}

pub(crate) fn print_openapi_warnings(warnings: &[OpenApiDiagnostic]) {
    for warning in warnings {
        eprintln!("warning: {warning}");
    }
}

pub(crate) fn format_duration(duration: Duration) -> String {
    if duration.as_secs() >= 1 {
        format!("{:.3}s", duration.as_secs_f64())
    } else if duration.as_millis() >= 1 {
        format!("{}ms", duration.as_millis())
    } else {
        format!("{}us", duration.as_micros())
    }
}

pub(crate) fn run_with_large_stack<T, F>(thread_name: &str, task: F) -> Result<T>
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
