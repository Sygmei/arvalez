//! Shared building blocks used by every Arvalez SDK generator backend.
//!
//! All three language targets (Go, Python, TypeScript) previously duplicated:
//! - The [`GeneratedFile`] type and [`write_files`] function
//! - [`load_templates`] (parameterised Tera setup)
//! - [`sorted_models`], [`sorted_operations`], [`operation_primary_tag`]
//! - [`indent_block`]
//! - [`ClientLayout`] (operations partitioned by tag)
//!
//! This crate provides the single canonical implementation for all of them.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use arvalez_ir::{CoreIr, Operation};
use serde::Serialize;
use tera::Tera;

// ── Output file ──────────────────────────────────────────────────────────────

/// A file to be written to disk as part of a generated SDK package.
#[derive(Debug, Clone, Serialize)]
pub struct GeneratedFile {
    pub path: PathBuf,
    pub contents: String,
}

/// Writes a slice of [`GeneratedFile`]s to `output_dir`, creating directories
/// as needed.
pub fn write_files(output_dir: impl AsRef<Path>, files: &[GeneratedFile]) -> Result<()> {
    let output_dir = output_dir.as_ref();
    fs::create_dir_all(output_dir).with_context(|| {
        format!("failed to create output directory `{}`", output_dir.display())
    })?;

    for file in files {
        let path = output_dir.join(&file.path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create directory `{}`", parent.display()))?;
        }
        fs::write(&path, &file.contents)
            .with_context(|| format!("failed to write `{}`", path.display()))?;
    }

    Ok(())
}

// ── Template loading ──────────────────────────────────────────────────────────

/// Initialises a [`Tera`] engine with `builtin_templates`, then overrides any
/// entry that appears in `overridable_templates` with a file found under
/// `template_dir` (if provided).
pub fn load_templates(
    template_dir: Option<&Path>,
    builtin_templates: &[(&str, &str)],
    overridable_templates: &[&str],
) -> Result<Tera> {
    let mut tera = Tera::default();
    for (name, contents) in builtin_templates {
        tera.add_raw_template(name, contents)
            .with_context(|| format!("failed to register builtin template `{name}`"))?;
    }

    if let Some(template_dir) = template_dir {
        for name in overridable_templates {
            let candidate = template_dir.join(name);
            if !candidate.exists() {
                continue;
            }
            let contents = fs::read_to_string(&candidate).with_context(|| {
                format!("failed to read template override `{}`", candidate.display())
            })?;
            tera.add_raw_template(name, &contents).with_context(|| {
                format!(
                    "failed to register template override `{}`",
                    candidate.display()
                )
            })?;
        }
    }

    Ok(tera)
}

// ── IR helpers ────────────────────────────────────────────────────────────────

/// Returns IR models sorted alphabetically by name.
pub fn sorted_models(ir: &CoreIr) -> Vec<&arvalez_ir::Model> {
    let mut models = ir.models.iter().collect::<Vec<_>>();
    models.sort_by(|l, r| l.name.cmp(&r.name));
    models
}

/// Returns IR operations sorted alphabetically by name.
pub fn sorted_operations(ir: &CoreIr) -> Vec<&Operation> {
    let mut ops = ir.operations.iter().collect::<Vec<_>>();
    ops.sort_by(|l, r| l.name.cmp(&r.name));
    ops
}

/// Returns the first non-empty tag of an operation (from the `tags` attribute),
/// or `None` if the operation carries no tags.
pub fn operation_primary_tag(operation: &Operation) -> Option<String> {
    operation
        .attributes
        .get("tags")
        .and_then(|v| v.as_array())
        .and_then(|tags| tags.first())
        .and_then(|tag| tag.as_str())
        .map(str::trim)
        .filter(|tag| !tag.is_empty())
        .map(ToOwned::to_owned)
}

// ── Text helpers ──────────────────────────────────────────────────────────────

/// Prepends `spaces` spaces to every entry in `lines` and joins them with
/// newlines.
pub fn indent_block(lines: &[String], spaces: usize) -> String {
    let indent = " ".repeat(spaces);
    lines
        .iter()
        .map(|line| format!("{indent}{line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

// ── Client layout ─────────────────────────────────────────────────────────────

/// Operations partitioned by tag for grouped (service-style) client layouts.
///
/// - [`all_operations`](ClientLayout::all_operations) — every operation sorted
///   by name.
/// - [`untagged_operations`](ClientLayout::untagged_operations) — operations
///   that carry no primary tag.
/// - [`tagged_groups`](ClientLayout::tagged_groups) — `(tag_name, operations)`
///   pairs sorted by tag name; per-group operations preserve the
///   `all_operations` order.
pub struct ClientLayout<'a> {
    pub all_operations: Vec<&'a Operation>,
    pub untagged_operations: Vec<&'a Operation>,
    pub tagged_groups: Vec<(String, Vec<&'a Operation>)>,
}

impl<'a> ClientLayout<'a> {
    pub fn from_ir(ir: &'a CoreIr) -> Self {
        let all_operations = sorted_operations(ir);
        let mut tag_map: BTreeMap<String, Vec<&Operation>> = BTreeMap::new();
        let mut untagged_operations = Vec::new();

        for operation in &all_operations {
            match operation_primary_tag(operation) {
                Some(tag) => tag_map.entry(tag).or_default().push(*operation),
                None => untagged_operations.push(*operation),
            }
        }

        let tagged_groups = tag_map.into_iter().collect();

        Self {
            all_operations,
            untagged_operations,
            tagged_groups,
        }
    }
}
