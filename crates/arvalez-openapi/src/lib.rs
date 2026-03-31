use std::{fs, time::Instant};
use std::path::Path;

use anyhow::{Context, Result};

mod diagnostic;
mod document;
mod importer;
mod merge;
mod naming;
mod parse;
mod schema;
mod source;
#[cfg(test)]
mod tests;

pub use diagnostic::{
    DiagnosticKind, OpenApiDiagnostic, OpenApiLoadResult,
    categorize_reference, diagnostic_pointer_tail, normalize_diagnostic_feature,
};

use importer::OpenApiImporter;
use parse::{parse_json_openapi_document, parse_yaml_openapi_document};

#[derive(Debug, Clone, Copy, Default)]
pub struct LoadOpenApiOptions {
    pub ignore_unhandled: bool,
    pub emit_timings: bool,
}

pub fn load_openapi_to_ir(path: impl AsRef<Path>) -> Result<arvalez_ir::CoreIr> {
    Ok(load_openapi_to_ir_with_options(path, LoadOpenApiOptions::default())?.ir)
}

pub fn load_openapi_to_ir_with_options(
    path: impl AsRef<Path>,
    options: LoadOpenApiOptions,
) -> Result<OpenApiLoadResult> {
    let path = path.as_ref();
    let raw = measure_openapi_phase(options.emit_timings, "openapi_read", || {
        fs::read_to_string(path)
            .with_context(|| format!("failed to read OpenAPI document `{}`", path.display()))
    })?;

    let loaded = measure_openapi_phase(options.emit_timings, "openapi_parse", || {
        match path.extension().and_then(|ext| ext.to_str()) {
            Some("yaml") | Some("yml") => parse_yaml_openapi_document(path, &raw),
            _ => parse_json_openapi_document(path, &raw),
        }
    })?;

    OpenApiImporter::new(loaded.document, loaded.source, options).build_ir()
}

pub(crate) fn measure_openapi_phase<T, F>(enabled: bool, label: &str, task: F) -> Result<T>
where
    F: FnOnce() -> Result<T>,
{
    if enabled {
        eprintln!("timing: starting {label}");
    }
    let started = Instant::now();
    let value = task();
    if enabled {
        eprintln!(
            "timing: {:<20} {}",
            label,
            format_duration(started.elapsed())
        );
    }
    value
}

pub(crate) fn format_duration(duration: std::time::Duration) -> String {
    let micros = duration.as_micros();
    if micros < 1_000 {
        format!("{micros}us")
    } else if micros < 1_000_000 {
        format!("{}ms", duration.as_millis())
    } else {
        format!("{:.2}s", duration.as_secs_f64())
    }
}
