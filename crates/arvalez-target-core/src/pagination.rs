//! Language-independent pagination configuration and schema resolution.
use std::collections::BTreeMap;

use anyhow::{Context, Result, bail, ensure};
use arvalez_ir::{CoreIr, HttpMethod, ModelKind, Operation, ParameterLocation, TypeRef};
use serde::{Deserialize, Serialize};

pub const PAGINATION: &str = "arvalez_pagination";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaginationStrategy {
    Page,
    Offset,
    Cursor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaginationStop {
    EmptyPage,
    ShortPage,
}

/// Fields are optional so operation overrides can inherit service defaults.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PaginationSettings {
    pub disabled: Option<bool>,
    pub strategy: Option<PaginationStrategy>,
    pub page_param: Option<String>,
    pub offset_param: Option<String>,
    pub cursor_param: Option<String>,
    pub page_size_param: Option<String>,
    pub start_page: Option<i64>,
    pub start_offset: Option<i64>,
    pub default_page_size: Option<i64>,
    pub max_page_size: Option<i64>,
    pub items_path: Option<String>,
    pub next_cursor_path: Option<String>,
    pub total_pages_path: Option<String>,
    pub total_items_path: Option<String>,
    pub has_more_path: Option<String>,
    pub last_page_path: Option<String>,
    pub stop_on: Option<PaginationStop>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PaginationConfig {
    #[serde(flatten)]
    pub defaults: PaginationSettings,
    #[serde(default)]
    pub operations: BTreeMap<String, PaginationSettings>,
}

impl PaginationSettings {
    fn has_stop(&self) -> bool {
        self.next_cursor_path.is_some()
            || self.total_pages_path.is_some()
            || self.total_items_path.is_some()
            || self.has_more_path.is_some()
            || self.last_page_path.is_some()
            || self.stop_on.is_some()
    }

    fn clear_stop(&mut self) {
        self.next_cursor_path = None;
        self.total_pages_path = None;
        self.total_items_path = None;
        self.has_more_path = None;
        self.last_page_path = None;
        self.stop_on = None;
    }

    /// Changing strategy clears strategy-specific inherited fields. Supplying a
    /// termination rule replaces the inherited rule as a unit.
    pub fn with_override(&self, other: &Self) -> Self {
        let mut result = self.clone();
        if other.strategy.is_some() && other.strategy != self.strategy {
            result.page_param = None;
            result.offset_param = None;
            result.cursor_param = None;
            result.start_page = None;
            result.start_offset = None;
            result.clear_stop();
        }
        if other.has_stop() {
            result.clear_stop();
        }
        macro_rules! merge {
            ($($field:ident),* $(,)?) => { $(
                if other.$field.is_some() { result.$field = other.$field.clone(); }
            )* };
        }
        merge!(
            disabled,
            strategy,
            page_param,
            offset_param,
            cursor_param,
            page_size_param,
            start_page,
            start_offset,
            default_page_size,
            max_page_size,
            items_path,
            next_cursor_path,
            total_pages_path,
            total_items_path,
            has_more_path,
            last_page_path,
            stop_on
        );
        result
    }

    fn validate(&self) -> Result<()> {
        if self.disabled == Some(true) {
            return Ok(());
        }
        let strategy = self.strategy.context("pagination strategy is required")?;
        let size = self
            .default_page_size
            .context("default_page_size is required")?;
        ensure!(size > 0, "default_page_size must be positive");
        if let Some(max) = self.max_page_size {
            ensure!(
                max > 0 && size <= max,
                "max_page_size must be positive and >= default_page_size"
            );
        }
        query_name(
            self.page_size_param
                .as_deref()
                .context("page_size_param is required")?,
        )?;
        pointer_tokens(
            self.items_path
                .as_deref()
                .context("items_path is required")?,
        )?;
        let paths = [
            &self.next_cursor_path,
            &self.total_pages_path,
            &self.total_items_path,
            &self.has_more_path,
            &self.last_page_path,
        ];
        let stops =
            paths.iter().filter(|p| p.is_some()).count() + usize::from(self.stop_on.is_some());
        ensure!(
            stops == 1,
            "configure exactly one pagination termination rule"
        );
        for path in paths.into_iter().flatten() {
            pointer_tokens(path)?;
        }
        let position = match strategy {
            PaginationStrategy::Page => {
                ensure!(
                    self.offset_param.is_none()
                        && self.cursor_param.is_none()
                        && self.start_offset.is_none()
                        && self.next_cursor_path.is_none(),
                    "page strategy contains incompatible settings"
                );
                ensure!(
                    self.start_page.unwrap_or(1) >= 0,
                    "start_page must be nonnegative"
                );
                self.page_param
                    .as_deref()
                    .context("page_param is required")?
            }
            PaginationStrategy::Offset => {
                ensure!(
                    self.page_param.is_none()
                        && self.cursor_param.is_none()
                        && self.start_page.is_none()
                        && self.next_cursor_path.is_none()
                        && self.total_pages_path.is_none()
                        && self.last_page_path.is_none(),
                    "offset strategy contains incompatible settings"
                );
                ensure!(
                    self.start_offset.unwrap_or(0) >= 0,
                    "start_offset must be nonnegative"
                );
                self.offset_param
                    .as_deref()
                    .context("offset_param is required")?
            }
            PaginationStrategy::Cursor => {
                ensure!(
                    self.page_param.is_none()
                        && self.offset_param.is_none()
                        && self.start_page.is_none()
                        && self.start_offset.is_none()
                        && self.next_cursor_path.is_some(),
                    "cursor strategy requires next_cursor_path and no page/offset settings"
                );
                self.cursor_param
                    .as_deref()
                    .context("cursor_param is required")?
            }
        };
        query_name(position)?;
        ensure!(
            Some(position) != self.page_size_param.as_deref(),
            "position and page size must use different parameters"
        );
        Ok(())
    }
}

/// Decode JSON Pointer tokens, rejecting malformed escapes (RFC 6901).
pub fn pointer_tokens(path: &str) -> Result<Vec<String>> {
    if path.is_empty() {
        return Ok(Vec::new());
    }
    ensure!(
        path.starts_with('/'),
        "JSON Pointer `{path}` must be empty or start with /"
    );
    path[1..]
        .split('/')
        .map(|part| {
            let mut chars = part.chars();
            let mut token = String::new();
            while let Some(ch) = chars.next() {
                token.push(if ch == '~' {
                    match chars.next() {
                        Some('0') => '~',
                        Some('1') => '/',
                        _ => bail!("invalid JSON Pointer escape in `{path}`"),
                    }
                } else {
                    ch
                });
            }
            Ok(token)
        })
        .collect()
}

fn query_name(value: &str) -> Result<&str> {
    value
        .strip_prefix("query.")
        .filter(|name| !name.is_empty())
        .context("pagination parameters must use query.<wire_name>")
}

fn schema_at(ir: &CoreIr, root: &TypeRef, path: &str) -> Result<TypeRef> {
    let mut ty = root.clone();
    for token in pointer_tokens(path)? {
        ty = match ty {
            TypeRef::Named { name } => {
                let model = ir
                    .models
                    .iter()
                    .find(|m| m.name == name)
                    .context("unknown response model")?;
                ensure!(
                    matches!(model.kind, ModelKind::Object),
                    "cannot traverse enum model {name}"
                );
                model
                    .fields
                    .iter()
                    .find(|f| f.name == token)
                    .with_context(|| format!("response field `{token}` not found in {name}"))?
                    .type_ref
                    .clone()
            }
            TypeRef::Map { value } => *value,
            TypeRef::Array { item } => {
                ensure!(
                    token == "0" || (!token.starts_with('0') && token.parse::<usize>().is_ok()),
                    "invalid array index `{token}`"
                );
                *item
            }
            _ => bail!("cannot traverse response path `{path}`"),
        };
    }
    Ok(ty)
}

#[derive(Clone, Debug, Serialize)]
pub struct ResolvedPagination {
    pub settings: PaginationSettings,
    pub item_type: TypeRef,
    pub position_index: usize,
    pub size_index: usize,
    pub position_name: String,
    pub size_name: String,
    pub start: i64,
    pub items_tokens: Vec<String>,
    pub stop_tokens: Vec<String>,
    pub stop_kind: String,
}

fn resolve_operation(
    ir: &CoreIr,
    op: &Operation,
    settings: PaginationSettings,
) -> Result<ResolvedPagination> {
    ensure!(
        op.method == HttpMethod::Get,
        "automatic pagination currently supports GET operations"
    );
    let strategy = settings.strategy.unwrap(); // validated before resolution
    let position = match strategy {
        PaginationStrategy::Page => settings.page_param.as_deref(),
        PaginationStrategy::Offset => settings.offset_param.as_deref(),
        PaginationStrategy::Cursor => settings.cursor_param.as_deref(),
    }
    .unwrap();
    let find_param = |selector: &str, expected: &str| -> Result<usize> {
        let name = query_name(selector)?;
        let (index, param) = op
            .params
            .iter()
            .enumerate()
            .find(|(_, p)| p.location == ParameterLocation::Query && p.name == name)
            .with_context(|| format!("missing pagination parameter `{selector}`"))?;
        ensure!(
            param.type_ref == TypeRef::primitive(expected),
            "pagination parameter `{selector}` must have type {expected}"
        );
        Ok(index)
    };
    let position_index = find_param(
        position,
        if strategy == PaginationStrategy::Cursor {
            "string"
        } else {
            "integer"
        },
    )?;
    if strategy == PaginationStrategy::Cursor {
        ensure!(
            !op.params[position_index].required,
            "cursor parameter must be optional for the initial request"
        );
    }
    let size_index = find_param(settings.page_size_param.as_deref().unwrap(), "integer")?;
    let responses: Vec<_> = op
        .responses
        .iter()
        .filter(|r| r.status.starts_with('2'))
        .collect();
    ensure!(
        !responses.is_empty(),
        "pagination requires a successful JSON response"
    );
    let (stop_kind, stop_path, stop_type) = if let Some(path) = &settings.next_cursor_path {
        ("cursor", Some(path), "string")
    } else if let Some(path) = &settings.total_pages_path {
        ("total_pages", Some(path), "integer")
    } else if let Some(path) = &settings.total_items_path {
        ("total_items", Some(path), "integer")
    } else if let Some(path) = &settings.has_more_path {
        ("has_more", Some(path), "boolean")
    } else if let Some(path) = &settings.last_page_path {
        ("last_page", Some(path), "integer")
    } else {
        (
            match settings.stop_on.unwrap() {
                PaginationStop::EmptyPage => "empty_page",
                PaginationStop::ShortPage => "short_page",
            },
            None,
            "",
        )
    };
    let mut item_type = None;
    for response in responses {
        let media = response
            .media_type
            .as_deref()
            .unwrap_or("")
            .split(';')
            .next()
            .unwrap_or("")
            .trim();
        ensure!(
            media == "application/json" || media.ends_with("+json"),
            "pagination requires JSON success responses"
        );
        let root = response
            .type_ref
            .as_ref()
            .context("pagination response has no body")?;
        let TypeRef::Array { item } = schema_at(ir, root, settings.items_path.as_deref().unwrap())?
        else {
            bail!("items_path must point to an array");
        };
        if let Some(previous) = &item_type {
            ensure!(
                previous == &*item,
                "success responses have different pagination item types"
            );
        }
        item_type = Some(*item);
        if let Some(path) = stop_path {
            ensure!(
                schema_at(ir, root, path)? == TypeRef::primitive(stop_type),
                "termination path `{path}` must have type {stop_type}"
            );
        }
    }
    Ok(ResolvedPagination {
        item_type: item_type.unwrap(),
        position_index,
        size_index,
        position_name: query_name(position)?.into(),
        size_name: query_name(settings.page_size_param.as_deref().unwrap())?.into(),
        start: if strategy == PaginationStrategy::Page {
            settings.start_page.unwrap_or(1)
        } else {
            settings.start_offset.unwrap_or(0)
        },
        items_tokens: pointer_tokens(settings.items_path.as_deref().unwrap())?,
        stop_tokens: stop_path
            .map(|p| pointer_tokens(p))
            .transpose()?
            .unwrap_or_default(),
        stop_kind: stop_kind.into(),
        settings,
    })
}

/// Service defaults apply only to compatible collection operations. Explicit
/// overrides are strict: unknown operations and incompatible schemas are errors.
pub fn resolve_pagination(
    ir: &CoreIr,
    config: &PaginationConfig,
) -> Result<BTreeMap<String, ResolvedPagination>> {
    if config.defaults.strategy.is_some() {
        config.defaults.validate().context("common.pagination")?;
    } else if serde_json::to_value(&config.defaults)?
        .as_object()
        .unwrap()
        .iter()
        .any(|(key, value)| key != "disabled" && !value.is_null())
    {
        bail!("common.pagination: strategy is required when defaults are configured");
    }
    for name in config.operations.keys() {
        ensure!(
            ir.operations.iter().any(|op| &op.name == name),
            "pagination override references unknown operation `{name}`"
        );
    }
    let mut result = BTreeMap::new();
    for op in &ir.operations {
        let explicit = config.operations.get(&op.name);
        let settings = explicit
            .map(|s| config.defaults.with_override(s))
            .unwrap_or_else(|| config.defaults.clone());
        if settings.disabled == Some(true) {
            continue;
        }
        if settings.strategy.is_none() && explicit.is_none() {
            continue;
        }
        settings
            .validate()
            .with_context(|| format!("pagination for {}", op.name))?;
        match resolve_operation(ir, op, settings) {
            Ok(resolved) => {
                result.insert(op.id.clone(), resolved);
            }
            Err(error) if explicit.is_some() => {
                return Err(error.context(format!("pagination for {}", op.name)));
            }
            Err(_) => {} // Defaults are a compatibility-based opt-in.
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests;

/// Shared Python pagination runtime used by both Python targets.
pub const PYTHON_PAGINATION_TEMPLATE: &str = include_str!("../templates/pagination.py.tera");
pub const PYTHON_PAGINATOR_TEMPLATE: &str = include_str!("../templates/paginator.py.tera");

/// Reject generated helper names that would overwrite existing operations.
pub fn validate_pagination_helper_names(
    operations: &serde_json::Value,
    sanitize: fn(&str) -> String,
    suffixes: &[&str],
    reserved: &[&str],
) -> tera::Result<bool> {
    let operations = operations.as_array().cloned().unwrap_or_default();
    let enabled = operations
        .iter()
        .any(|op| !op["attributes"][PAGINATION].is_null());
    if !enabled {
        return Ok(false);
    }
    let mut names: std::collections::HashSet<String> = operations
        .iter()
        .map(|op| sanitize(op["name"].as_str().unwrap_or_default()))
        .collect();
    for name in reserved {
        if !names.insert((*name).into()) {
            return Err(tera::Error::msg(format!(
                "pagination runtime name collision: {name}"
            )));
        }
    }
    for op in operations
        .iter()
        .filter(|op| !op["attributes"][PAGINATION].is_null())
    {
        let name = sanitize(op["name"].as_str().unwrap_or_default());
        for suffix in suffixes {
            let helper = format!("{name}{suffix}");
            if !names.insert(helper.clone()) {
                return Err(tera::Error::msg(format!(
                    "pagination helper name collision: {helper}"
                )));
            }
        }
    }
    Ok(true)
}

pub const PYTHON_PAGINATION_CLASS_TEMPLATE: &str =
    include_str!("../templates/pagination_class.py.tera");
