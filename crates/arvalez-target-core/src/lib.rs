//! Shared building blocks used by every Arvalez SDK generator backend.
//!
//! All three language targets (Go, Python, TypeScript) previously duplicated:
//! - The [`GeneratedFile`] type and [`write_files`] function
//! - [`load_templates`] (parameterised Tera setup)
//! - [`sorted_models`], [`sorted_operations`], [`operation_primary_tag`]
//! - [`indent_block`]
//! - [`ClientLayout`] (operations partitioned by tag)
//! - [`split_words`], [`to_snake_case`], [`to_pascal_case`] (identifier casing)
//!
//! This crate provides the single canonical implementation for all of them.

use std::{
    collections::{BTreeMap, HashMap},
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use arvalez_ir::{CoreIr, Model, Operation};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tera::Tera;

// Re-exported so the `declare_target!` macro can use `$crate::` paths without
// requiring callers to add these crates as direct dependencies.
pub use anyhow;
pub use arvalez_ir;
pub use serde_json;
pub use tera;

// ── Common config ─────────────────────────────────────────────────────────────

/// Config fields shared across all targets. Injected into the Tera context
/// before target-specific fields (which can override them).
///
/// - `package_name` is also expanded as `{package_name}` in output file paths.
/// - `version` is injected as `{{ version }}` in templates.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommonConfig {
    pub package_name: String,
    pub version: String,
}

impl Default for CommonConfig {
    fn default() -> Self {
        Self {
            package_name: "client".into(),
            version: "0.1.0".into(),
        }
    }
}

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
        format!(
            "failed to create output directory `{}`",
            output_dir.display()
        )
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
pub fn load_templates(template_dir: Option<&Path>, templates: &[(&str, &str)]) -> Result<Tera> {
    let mut tera = Tera::default();
    register_casing_filters(&mut tera);
    for (name, contents) in templates {
        tera.add_raw_template(name, contents)
            .with_context(|| format!("failed to register builtin template `{name}`"))?;
    }

    if let Some(template_dir) = template_dir {
        for (name, _) in templates {
            // Strip `{var}` path components so users place override files at the
            // normalized path, e.g. `root/src/__init__.py.tera` rather than
            // `root/src/{package_name}/__init__.py.tera`.
            let disk_name: String = name
                .split('/')
                .filter(|s| !(s.starts_with('{') && s.ends_with('}')))
                .collect::<Vec<_>>()
                .join("/");
            let candidate = template_dir.join(&disk_name);
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

/// Scans `template_dir` for tilde-prefixed eraser files that suppress generation
/// of a built-in template, using the same `{var}`-stripping normalization as the
/// override lookup in [`load_templates`].
///
/// For each template name (e.g. `root/src/{package_name}/models.py.tera`), the
/// disk path is normalized to `root/src/models.py.tera` and an eraser file
/// `root/src/~models.py.tera` is looked up. If found, the template name is
/// added to the returned list. Pass the result as `erased` to
/// [`render_root_templates`] to skip those files.
pub fn collect_erased_root_templates(
    template_dir: &Path,
    templates: &[(&str, &str)],
) -> Vec<String> {
    templates
        .iter()
        .filter_map(|(name, _)| {
            let disk_name: String = name
                .split('/')
                .filter(|s| !(s.starts_with('{') && s.ends_with('}')))
                .collect::<Vec<_>>()
                .join("/");
            let p = Path::new(&disk_name);
            let parent = p.parent()?;
            let filename = p.file_name()?.to_str()?;
            let eraser = template_dir.join(parent).join(format!("~{filename}"));
            if eraser.exists() {
                Some(name.to_string())
            } else {
                None
            }
        })
        .collect()
}

/// Render all `root/`-prefixed entries in `templates` into [`GeneratedFile`]s.
///
/// Core automatically inserts `models`, `operations` (sorted), `config`, and all
/// fields of `common` and `config_val` into the Tera context. `common` fields
/// (`package_name`, `version`) are injected first; target-specific `config_val`
/// string fields are injected on top and override common fields if present.
///
/// Both `common` and `config_val` string fields are expanded as `{key}`
/// placeholders in output paths (common first, target config wins).
///
/// Templates whose name appears in `erased` (see [`collect_erased_root_templates`]) are skipped.
/// Templates *not* under `root/` (e.g. `partials/`) are always skipped.
pub fn render_root_templates(
    tera: &Tera,
    ir: &CoreIr,
    mut ctx: tera::Context,
    templates: &[(&str, &str)],
    common: &CommonConfig,
    config_val: &Value,
    erased: &[String],
) -> Result<Vec<GeneratedFile>> {
    ctx.insert("models", &serde_json::to_value(sorted_models(ir))?);
    ctx.insert("operations", &serde_json::to_value(sorted_operations(ir))?);
    // Common fields go in first, then target config fields override them.
    ctx.insert("package_name", &common.package_name);
    ctx.insert("version", &common.version);
    ctx.insert("config", config_val);
    if let Some(obj) = config_val.as_object() {
        for (k, v) in obj {
            ctx.insert(k.as_str(), v);
        }
    }
    templates
        .iter()
        .filter_map(|(name, _)| name.strip_prefix("root/"))
        .filter(|rel| !erased.contains(&format!("root/{rel}")))
        .map(|rel| {
            let tpl_name = format!("root/{rel}");
            // Path expansion: common fields first, target config fields override.
            let base = rel.strip_suffix(".tera").unwrap_or(rel).to_string();
            let after_common = base
                .replace("{package_name}", &common.package_name)
                .replace("{version}", &common.version);
            let out_path = if let Some(obj) = config_val.as_object() {
                obj.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.as_str(), s)))
                    .fold(after_common, |acc, (k, v)| {
                        acc.replace(&format!("{{{k}}}"), v)
                    })
            } else {
                after_common
            };
            let contents = tera
                .render(&tpl_name, &ctx)
                .with_context(|| format!("failed to render {tpl_name}"))?;
            Ok(GeneratedFile {
                path: PathBuf::from(out_path),
                contents,
            })
        })
        .collect()
}

/// Declare a minimal SDK generator target.
///
/// Generates:
/// - `pub fn generate(ir, template_dir, config: &ConfigType) -> Result<Vec<GeneratedFile>>`
/// - `pub use write_files as write_package`
///
/// The config type must implement [`serde::Serialize`]. Its string fields are
/// automatically injected into the Tera context and expanded in output file paths.
///
/// # Example
/// ```rust,ignore
/// arvalez_target_core::declare_target! {
///     config:    MyTargetConfig,
///     templates: TEMPLATES,
///     filters:   register_filters,
/// }
/// ```
#[macro_export]
macro_rules! declare_target {
    (config: $config_ty:ty, templates: $templates:expr, filters: $filters:expr $(,)?) => {
        /// Generate all output files for the given IR and config.
        ///
        /// - `template_dir` overrides built-in templates found on disk.
        /// - Config fields are injected into Tera context and expanded in output paths.
        pub fn generate(
            ir: &$crate::arvalez_ir::CoreIr,
            template_dir: ::std::option::Option<&::std::path::Path>,
            common: &$crate::CommonConfig,
            config: &$config_ty,
        ) -> $crate::anyhow::Result<::std::vec::Vec<$crate::GeneratedFile>> {
            let mut tera = $crate::load_templates(template_dir, $templates)?;
            $filters(&mut tera);
            let config_val = $crate::serde_json::to_value(config)?;
            let erased = match template_dir {
                Some(dir) => $crate::collect_erased_root_templates(dir, $templates),
                None => ::std::vec::Vec::new(),
            };
            $crate::render_root_templates(
                &tera,
                ir,
                $crate::tera::Context::new(),
                $templates,
                common,
                &config_val,
                &erased,
            )
        }

        pub use $crate::write_files as write_package;

        /// Dump empty tilde-prefixed eraser files to `output_dir`, one per
        /// built-in template. Placing any of these files in your
        /// `--template-dir` suppresses generation of the corresponding output
        /// file. The layout matches the normalized (no `{var}`) disk paths.
        pub fn dump_erasers(output_dir: &::std::path::Path) -> $crate::anyhow::Result<()> {
            use $crate::anyhow::Context as _;
            for (name, _) in $templates {
                let disk_name: ::std::string::String = name
                    .split('/')
                    .filter(|s| !(s.starts_with('{') && s.ends_with('}')))
                    .collect::<::std::vec::Vec<_>>()
                    .join("/");
                let p = ::std::path::Path::new(&disk_name);
                if let (Some(parent), Some(filename)) = (p.parent(), p.file_name()) {
                    let eraser = output_dir
                        .join(parent)
                        .join(format!("~{}", filename.to_string_lossy()));
                    if let Some(eraser_parent) = eraser.parent() {
                        ::std::fs::create_dir_all(eraser_parent).with_context(|| {
                            format!("failed to create directory `{}`", eraser_parent.display())
                        })?;
                    }
                    ::std::fs::write(&eraser, b"").with_context(|| {
                        format!("failed to write eraser `{}`", eraser.display())
                    })?;
                }
            }
            Ok(())
        }

        /// Dump all built-in templates to `output_dir`, preserving the directory
        /// structure. `{var}` placeholder path components are stripped so the
        /// layout matches what `generate` expects for `--template-dir` overrides.
        pub fn dump_templates(output_dir: &::std::path::Path) -> $crate::anyhow::Result<()> {
            use $crate::anyhow::Context as _;
            for (name, contents) in $templates {
                // Strip `{var}` components — same normalization as override lookup.
                let disk_name: ::std::string::String = name
                    .split('/')
                    .filter(|s| !(s.starts_with('{') && s.ends_with('}')))
                    .collect::<::std::vec::Vec<_>>()
                    .join("/");
                let dest = output_dir.join(&disk_name);
                if let Some(parent) = dest.parent() {
                    ::std::fs::create_dir_all(parent).with_context(|| {
                        format!("failed to create directory `{}`", parent.display())
                    })?;
                }
                ::std::fs::write(&dest, contents)
                    .with_context(|| format!("failed to write template `{}`", dest.display()))?;
            }
            Ok(())
        }
    };
}

// ── Casing Tera filters ───────────────────────────────────────────────────────

/// Registers language-agnostic casing filters into `tera`.
///
/// Called automatically by [`load_templates`]; targets do not need to call
/// this explicitly.
///
/// | filter name            | input        | output              |
/// |------------------------|--------------|---------------------|
/// | `snake_case`           | `str`        | `some_name`         |
/// | `pascal_case`          | `str`        | `SomeName`          |
/// | `screaming_snake_case` | `str` / any  | `SOME_NAME`         |
pub fn register_casing_filters(tera: &mut Tera) {
    tera.register_filter("snake_case", |v: &Value, _: &HashMap<String, Value>| {
        Ok(Value::String(to_snake_case(v.as_str().unwrap_or(""))))
    });
    tera.register_filter("pascal_case", |v: &Value, _: &HashMap<String, Value>| {
        Ok(Value::String(to_pascal_case(v.as_str().unwrap_or(""))))
    });
    tera.register_filter(
        "screaming_snake_case",
        |v: &Value, _: &HashMap<String, Value>| {
            let name = match v {
                Value::String(s) => {
                    let upper = to_screaming_snake_case(s);
                    if upper.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                        format!("_{upper}")
                    } else {
                        upper
                    }
                }
                Value::Number(n) => format!("VALUE_{n}"),
                Value::Bool(b) => {
                    if *b {
                        "TRUE".into()
                    } else {
                        "FALSE".into()
                    }
                }
                _ => "VALUE".into(),
            };
            Ok(Value::String(name))
        },
    );
}

// ── Erased template detection ────────────────────────────────────────────────

/// Scans `template_dir` for tilde-prefixed eraser files that suppress the
/// generation of a default template.
///
/// For each name in `overridable_templates` (e.g. `package/pyproject.toml.tera`),
/// if a file with a `~` prepended to the filename component exists under
/// `template_dir` (e.g. `template_dir/package/~pyproject.toml.tera`), that
/// template name is added to the returned list.
///
/// Callers should skip rendering any template whose name appears in the
/// returned list, so that the corresponding output file is not produced.
pub fn collect_erased_templates(
    template_dir: &Path,
    overridable_templates: &[&str],
) -> Vec<String> {
    overridable_templates
        .iter()
        .filter(|&&name| {
            let p = Path::new(name);
            match (p.parent(), p.file_name()) {
                (Some(parent), Some(filename)) => {
                    let eraser_name = format!("~{}", filename.to_string_lossy());
                    template_dir.join(parent).join(&eraser_name).exists()
                }
                _ => false,
            }
        })
        .map(|&name| name.to_owned())
        .collect()
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
            tera.add_raw_template(&template_name, &contents)
                .with_context(|| format!("failed to compile extra template `{template_name}`"))?;
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

/// Splits an identifier string into words, correctly handling camelCase,
/// PascalCase, SCREAMING_SNAKE, acronyms (e.g. `HTTPHeader` → `["HTTP", "Header"]`),
/// digit boundaries, and separator characters.
///
/// # Examples
/// ```
/// use arvalez_target_core::split_words;
/// assert_eq!(split_words("HTTPHeader"),  vec!["HTTP", "Header"]);
/// assert_eq!(split_words("userId"),       vec!["user", "Id"]);
/// assert_eq!(split_words("some_value"),   vec!["some", "value"]);
/// ```
pub fn split_words(input: &str) -> Vec<String> {
    let chars = input.chars().collect::<Vec<_>>();
    let mut words = Vec::new();
    let mut current = String::new();
    for (index, ch) in chars.iter().copied().enumerate() {
        if ch.is_ascii_alphanumeric() {
            let previous = index.checked_sub(1).and_then(|i| chars.get(i)).copied();
            let next = chars.get(index + 1).copied();
            let next_next = chars.get(index + 2).copied();
            let should_split = !current.is_empty()
                && previous.is_some_and(|prev| {
                    (prev.is_ascii_lowercase() && ch.is_ascii_uppercase())
                        || (prev.is_ascii_digit() && ch.is_ascii_alphabetic())
                        || (prev.is_ascii_uppercase()
                            && ch.is_ascii_uppercase()
                            && (current.len() > 1
                                || (current.len() == 1
                                    && next_next.is_some_and(|c| c.is_ascii_lowercase())))
                            && next.is_some_and(|c| c.is_ascii_lowercase()))
                });
            if should_split {
                words.push(current.clone());
                current.clear();
            }
            current.push(ch);
        } else if !current.is_empty() {
            words.push(current.clone());
            current.clear();
        }
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

/// Converts an identifier to `snake_case` (lowercased words joined by `_`).
///
/// Does **not** escape language keywords — callers are responsible for that.
pub fn to_snake_case(s: &str) -> String {
    split_words(s)
        .iter()
        .map(|w| w.to_ascii_lowercase())
        .collect::<Vec<_>>()
        .join("_")
}

/// Like [`to_snake_case`] but also prefixes the result with `_` when it starts
/// with an ASCII digit, making it a syntactically valid identifier in most
/// languages (C, Go, Python, TypeScript, …).
///
/// Does **not** escape language keywords — callers are responsible for that.
pub fn to_snake_identifier(s: &str) -> String {
    let mut result = to_snake_case(s);
    if result.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        result.insert(0, '_');
    }
    result
}

/// Converts an identifier to `PascalCase`, preserving all-uppercase acronym
/// words (e.g. `"HTTP"`, `"API"`) as-is.
///
/// Does **not** escape language keywords — callers are responsible for that.
pub fn to_pascal_case(s: &str) -> String {
    let result = split_words(s)
        .iter()
        .map(|w| {
            // If the whole word is uppercase letters (possibly with digits), keep it.
            if w.len() > 1
                && w.chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
            {
                w.clone()
            } else {
                let mut c = w.chars();
                c.next()
                    .map(|f| {
                        f.to_uppercase().collect::<String>() + &c.as_str().to_ascii_lowercase()
                    })
                    .unwrap_or_default()
            }
        })
        .collect::<String>();
    if result.is_empty() {
        "GeneratedModel".into()
    } else if result.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        format!("_{result}")
    } else {
        result
    }
}

/// Converts an identifier to `SCREAMING_SNAKE_CASE`.
pub fn to_screaming_snake_case(s: &str) -> String {
    let upper = split_words(s)
        .iter()
        .map(|w| w.to_ascii_uppercase())
        .collect::<Vec<_>>()
        .join("_");
    if upper.is_empty() {
        "VALUE".into()
    } else {
        upper
    }
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
