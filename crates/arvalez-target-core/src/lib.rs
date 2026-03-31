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
use arvalez_ir::{CoreIr, Model, Operation};
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

// ── Language target trait ──────────────────────────────────────────────────────

/// A language backend that can emit code from individual IR elements.
///
/// This is the central extension point for new target languages. Implement
/// this trait for a generator struct that holds a [`Tera`] engine and the
/// target-specific config. Each method corresponds to one kind of IR element
/// and renders it to a target-language code snippet using the target's
/// built-in (or user-overridden) Tera templates.
///
/// The blanket `generate` method drives the full SDK generation workflow:
/// call it instead of the lower-level methods when you want to produce all
/// output files at once.
///
/// # Example
/// ```rust,ignore
/// let generator = NushellGenerator::new(&config)?;
/// let files = generator.generate(&ir)?;
/// write_files("./out", &files)?;
/// ```
pub trait IrEmitter {
    /// Render a single IR model to a target-language code snippet.
    ///
    /// The whole `ir` is provided so implementations can build cross-model
    /// context (e.g. a type registry) when needed.
    fn emit_model(&self, ir: &CoreIr, model: &Model) -> Result<String>;

    /// Render a single IR operation to a target-language code snippet.
    ///
    /// The whole `ir` is provided for the same reason as [`emit_model`].
    ///
    /// [`emit_model`]: IrEmitter::emit_model
    fn emit_operation(&self, ir: &CoreIr, operation: &Operation) -> Result<String>;

    /// Generate the complete set of output files from the entire IR.
    ///
    /// This is the primary entry point for SDK generation. Implementations
    /// assemble the full package (all files, package metadata, etc.) from the
    /// IR elements rendered by `emit_model`/`emit_operation` and the
    /// top-level package templates.
    fn generate(&self, ir: &CoreIr) -> Result<Vec<GeneratedFile>>;
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

// ── Extra template discovery ──────────────────────────────────────────────────

/// Scans `template_dir/package/` recursively for any `.tera` files whose
/// template name (path relative to `template_dir`, forward-slash separated)
/// does **not** appear in `known_names`.  Each discovered file is compiled
/// into `tera` and the function returns a `Vec<(template_name, output_path)>`
/// for the caller to render.
///
/// # Output path convention
///
/// The `output_path` in each pair is derived by stripping the `package/`
/// prefix and the `.tera` extension from the relative path:
///
/// ```text
/// template_dir/package/my_util.py.tera  →  my_util.py
/// template_dir/package/sub/dir/foo.go.tera  →  sub/dir/foo.go
/// ```
///
/// # Template context
///
/// Extra templates receive exactly the same context variable (`package`) as
/// all builtin templates.  The available fields depend on the target:
///
/// | field | Python | Go | TypeScript | Nushell |
/// |---|---|---|---|---|
/// | `package.package_name` | ✓ | ✓ | ✓ | ✓ |
/// | `package.version` | ✓ | ✓ | ✓ | ✓ |
/// | `package.model_blocks` | ✓ | ✓ | ✓ | ✓ |
/// | `package.client_blocks` / service / tag blocks | ✓ | ✓ | ✓ | ✓ |
///
/// Returns an empty `Vec` if `template_dir/package/` does not exist.
pub fn load_extra_package_templates(
    template_dir: &Path,
    known_names: &[&str],
    tera: &mut Tera,
) -> Result<Vec<(String, PathBuf)>> {
    let scan_dir = template_dir.join("package");
    if !scan_dir.exists() {
        return Ok(Vec::new());
    }
    let mut extras = Vec::new();
    collect_extra_templates_recursive(&scan_dir, template_dir, known_names, tera, &mut extras)?;
    extras.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(extras)
}

fn collect_extra_templates_recursive(
    dir: &Path,
    template_root: &Path,
    known_names: &[&str],
    tera: &mut Tera,
    out: &mut Vec<(String, PathBuf)>,
) -> Result<()> {
    for entry in fs::read_dir(dir)
        .with_context(|| format!("failed to read directory `{}`", dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_extra_templates_recursive(&path, template_root, known_names, tera, out)?;
        } else if path.extension().and_then(|e| e.to_str()) == Some("tera") {
            let rel = path
                .strip_prefix(template_root)
                .expect("scanned path must be under template_root");
            let template_name = rel
                .to_str()
                .with_context(|| format!("non-UTF-8 template path `{}`", rel.display()))?
                .replace('\\', "/");
            if known_names.contains(&template_name.as_str()) {
                continue; // already handled by a builtin/override
            }
            let contents = fs::read_to_string(&path)
                .with_context(|| format!("failed to read extra template `{}`", path.display()))?;
            tera.add_raw_template(&template_name, &contents).with_context(|| {
                format!("failed to compile extra template `{template_name}`")
            })?;
            // Derive output path: strip "package/" prefix and ".tera" suffix.
            let output_rel = rel
                .strip_prefix("package")
                .expect("extra templates are always discovered under package/");
            let output_str = output_rel
                .to_str()
                .expect("already validated as UTF-8")
                .trim_end_matches(".tera");
            out.push((template_name, PathBuf::from(output_str)));
        }
    }
    Ok(())
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
