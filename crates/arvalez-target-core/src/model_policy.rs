//! Response-only model projections. Source IR and request representations are preserved.
use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Result, bail};
use arvalez_ir::{CoreIr, Model, TypeRef};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const ALWAYS_PRESENT: &str = "arvalez_response_always_present";
pub const SOURCE_MODEL: &str = "arvalez_response_source_model";
pub const RECURSIVE_POINTER: &str = "arvalez_recursive_pointer";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResponseFields {
    #[default]
    Schema,
    AlwaysPresent,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelOverride {
    pub response_fields: Option<ResponseFields>,
}

/// Common defaults or target overrides. Resolve the two layers before generation.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelPolicies {
    pub response_fields: Option<ResponseFields>,
    #[serde(default)]
    pub overrides: BTreeMap<String, ModelOverride>,
}

impl ModelPolicies {
    /// Model-specific settings take precedence over target-wide defaults.
    pub fn resolve(common: &Self, target: &Self) -> Self {
        let mut overrides = common.overrides.clone();
        for (name, value) in &target.overrides {
            if value.response_fields.is_some() {
                overrides.insert(name.clone(), value.clone());
            } else {
                overrides.entry(name.clone()).or_default();
            }
        }
        Self {
            response_fields: target.response_fields.or(common.response_fields),
            overrides,
        }
    }

    pub fn for_model(&self, name: &str) -> ResponseFields {
        self.overrides
            .get(name)
            .and_then(|v| v.response_fields)
            .or(self.response_fields)
            .unwrap_or_default()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelTarget {
    Go,
    TypeScript,
    Python,
    Nushell,
}

fn references(ty: &TypeRef, out: &mut BTreeSet<String>) {
    match ty {
        TypeRef::Named { name } => {
            out.insert(name.clone());
        }
        TypeRef::Enum { base, .. } => references(base, out),
        TypeRef::Array { item } => references(item, out),
        TypeRef::Map { value } => references(value, out),
        TypeRef::Union { variants } => {
            for ty in variants {
                references(ty, out);
            }
        }
        TypeRef::Tuple { items, rest } => {
            for ty in items {
                references(ty, out);
            }
            if let arvalez_ir::TupleRest::Typed { item } = rest {
                references(item, out);
            }
        }
        TypeRef::Primitive { .. } => {}
    }
}

fn model_references(model: &Model) -> Result<BTreeSet<String>> {
    let mut refs = BTreeSet::new();
    for field in &model.fields {
        references(&field.type_ref, &mut refs);
    }
    if let Some(alias) = model.attributes.get("alias_type_ref") {
        references(&serde_json::from_value(alias.clone())?, &mut refs);
    }
    if let arvalez_ir::ModelKind::Enum { base, .. } = &model.kind {
        references(base, &mut refs);
    }
    Ok(refs)
}

fn closure(
    graph: &BTreeMap<String, BTreeSet<String>>,
    roots: BTreeSet<String>,
) -> BTreeSet<String> {
    let mut seen = roots;
    let mut pending: Vec<_> = seen.iter().cloned().collect();
    while let Some(name) = pending.pop() {
        if let Some(children) = graph.get(&name) {
            for child in children {
                if seen.insert(child.clone()) {
                    pending.push(child.clone());
                }
            }
        }
    }
    seen
}

fn rewrite(ty: &mut TypeRef, names: &BTreeMap<String, String>) {
    match ty {
        TypeRef::Named { name } => {
            if let Some(new) = names.get(name) {
                *name = new.clone();
            }
        }
        TypeRef::Enum { base, .. } => rewrite(base, names),
        TypeRef::Array { item } => rewrite(item, names),
        TypeRef::Map { value } => rewrite(value, names),
        TypeRef::Union { variants } => {
            for ty in variants {
                rewrite(ty, names);
            }
        }
        TypeRef::Tuple { items, rest } => {
            for ty in items {
                rewrite(ty, names);
            }
            if let arvalez_ir::TupleRest::Typed { item } = rest {
                rewrite(item, names);
            }
        }
        TypeRef::Primitive { .. } => {}
    }
}

/// Produce a target's response view before filtering operations. This function
/// deliberately leaves unreachable models intact and protects their references.
pub fn prepare_response_models(
    ir: &CoreIr,
    policies: &ModelPolicies,
    target: ModelTarget,
    identifier: fn(&str) -> String,
) -> Result<CoreIr> {
    let models: BTreeMap<_, _> = ir.models.iter().map(|m| (m.name.clone(), m)).collect();
    for name in policies.overrides.keys() {
        if !models.contains_key(name) {
            bail!("unknown model policy override {name:?}");
        }
    }
    if !ir
        .models
        .iter()
        .any(|m| policies.for_model(&m.name) == ResponseFields::AlwaysPresent)
    {
        return Ok(ir.clone());
    }
    let graph: BTreeMap<_, _> = ir
        .models
        .iter()
        .map(|m| Ok((m.name.clone(), model_references(m)?)))
        .collect::<Result<_>>()?;
    let mut response_roots = BTreeSet::new();
    let mut request_roots = BTreeSet::new();
    for op in &ir.operations {
        for p in &op.params {
            references(&p.type_ref, &mut request_roots);
        }
        if let Some(ty) = op.request_body.as_ref().and_then(|b| b.type_ref.as_ref()) {
            references(ty, &mut request_roots);
        }
        for r in &op.responses {
            if let Some(ty) = &r.type_ref {
                references(ty, &mut response_roots);
            }
        }
    }
    let responses = closure(&graph, response_roots);
    request_roots.extend(
        models
            .keys()
            .filter(|name| !responses.contains(*name))
            .cloned(),
    );
    let protected = closure(&graph, request_roots);
    let mut changed: BTreeSet<_> = responses
        .iter()
        .filter(|name| {
            policies.for_model(name) == ResponseFields::AlwaysPresent
                && models.get(*name).is_some_and(|m| {
                    m.fields.iter().any(|f| {
                        f.optional
                            || match target {
                                ModelTarget::Go | ModelTarget::Nushell => f.nullable,
                                ModelTarget::Python => f.attributes.contains_key("default"),
                                ModelTarget::TypeScript => false,
                            }
                    })
                })
        })
        .cloned()
        .collect();
    loop {
        let parents: Vec<_> = responses
            .iter()
            .filter(|name| {
                !changed.contains(*name)
                    && graph.get(*name).is_some_and(|children| {
                        children
                            .iter()
                            .any(|child| changed.contains(child) && protected.contains(child))
                    })
            })
            .cloned()
            .collect();
        if parents.is_empty() {
            break;
        }
        changed.extend(parents);
    }
    let reserved = [
        "Client",
        "ApiClient",
        "APIError",
        "ClientOptions",
        "RequestOptions",
        "RequestOption",
        "ErrorHandler",
        "Nullable",
        "NewNullable",
        "Null",
        "WithRequestOptions",
        "SDKVersion",
        "JsonValue",
        "JsonPrimitive",
        "BaseModel",
        "Field",
        "ConfigDict",
        "Enum",
        "TypeAlias",
    ];
    let mut used: BTreeSet<_> = models.keys().map(|n| identifier(n)).collect();
    used.extend(reserved.iter().map(|n| identifier(n)));
    fn reserve_enums(value: &Value, identifier: fn(&str) -> String, used: &mut BTreeSet<String>) {
        match value {
            Value::Object(object) => {
                if object.get("kind").and_then(Value::as_str) == Some("enum")
                    && let Some(name) = object.get("name").and_then(Value::as_str)
                {
                    used.insert(identifier(name));
                }
                for child in object.values() {
                    reserve_enums(child, identifier, used);
                }
            }
            Value::Array(values) => {
                for child in values {
                    reserve_enums(child, identifier, used);
                }
            }
            _ => {}
        }
    }
    reserve_enums(&serde_json::to_value(ir)?, identifier, &mut used);
    if target == ModelTarget::Nushell {
        for op in &ir.operations {
            if let Some(name) = identifier(&op.name).strip_prefix("make-") {
                used.insert(name.to_owned());
            }
        }
    }
    if target == ModelTarget::Go || target == ModelTarget::TypeScript {
        let input_names =
            crate::operation_input_type_names(&serde_json::to_value(ir)?, identifier, &reserved);
        used.extend(
            input_names
                .as_object()
                .into_iter()
                .flat_map(|m| m.values())
                .filter_map(Value::as_str)
                .map(str::to_owned),
        );
        for op in &ir.operations {
            if let Some(tag) = op
                .attributes
                .get("tags")
                .and_then(Value::as_array)
                .and_then(|a| a.first())
                .and_then(Value::as_str)
            {
                used.insert(format!("{}Service", identifier(tag)));
            }
        }
    }
    let mut names = BTreeMap::new();
    for name in changed.intersection(&protected) {
        let base = format!("{name}Response");
        let mut candidate = base.clone();
        let mut suffix = 2;
        while !used.insert(identifier(&candidate)) {
            candidate = format!("{base}{suffix}");
            suffix += 1;
        }
        names.insert(name.clone(), candidate);
    }
    let mut result = ir.clone();
    let mut projected = Vec::new();
    for model in &mut result.models {
        if !responses.contains(&model.name) {
            continue;
        }
        if protected.contains(&model.name) && !changed.contains(&model.name) {
            continue;
        }
        let source = model.name.clone();
        let mut view = model.clone();
        if let Some(name) = names.get(&source) {
            view.name = name.clone();
            // Model IDs are opaque, but must remain unique independently of names.
            let base = format!("{}.response", model.id);
            view.id = base.clone();
            let mut suffix = 2;
            while ir
                .models
                .iter()
                .chain(projected.iter())
                .any(|m| m.id == view.id)
            {
                view.id = format!("{base}{suffix}");
                suffix += 1;
            }
            view.attributes
                .insert(SOURCE_MODEL.into(), Value::String(source.clone()));
        }
        let always = policies.for_model(&source) == ResponseFields::AlwaysPresent;
        for field in &mut view.fields {
            rewrite(&mut field.type_ref, &names);
            if always {
                field.optional = false;
                field
                    .attributes
                    .insert(ALWAYS_PRESENT.into(), Value::Bool(true));
            }
        }
        if let Some(alias) = view.attributes.get_mut("alias_type_ref") {
            let mut ty: TypeRef = serde_json::from_value(alias.clone())?;
            rewrite(&mut ty, &names);
            *alias = serde_json::to_value(ty)?;
        }
        if let arvalez_ir::ModelKind::Enum { base, .. } = &mut view.kind {
            rewrite(base, &names);
        }
        if names.contains_key(&source) {
            projected.push(view);
        } else {
            *model = view;
        }
    }
    result.models.extend(projected);
    for op in &mut result.operations {
        for response in &mut op.responses {
            if let Some(ty) = &mut response.type_ref {
                rewrite(ty, &names);
            }
        }
    }
    if target == ModelTarget::Go {
        // Only direct object-value edges need indirection. Slices and maps
        // already break size cycles and must retain their native representation.
        let value_graph: BTreeMap<_, BTreeSet<_>> = result
            .models
            .iter()
            .map(|m| {
                let children = m
                    .fields
                    .iter()
                    .filter(|f| !f.optional && !f.nullable)
                    .filter_map(|f| {
                        if let TypeRef::Named { name } = &f.type_ref {
                            Some(name.clone())
                        } else {
                            None
                        }
                    })
                    .collect();
                (m.name.clone(), children)
            })
            .collect();
        for model in &mut result.models {
            for field in &mut model.fields {
                if field.attributes.get(ALWAYS_PRESENT) != Some(&Value::Bool(true))
                    || field.nullable
                {
                    continue;
                }
                if let TypeRef::Named { name } = &field.type_ref
                    && closure(&value_graph, BTreeSet::from([name.clone()])).contains(&model.name)
                {
                    field
                        .attributes
                        .insert(RECURSIVE_POINTER.into(), Value::Bool(true));
                }
            }
        }
    }
    Ok(result)
}

/// Prepare models using the complete operation graph, then filter emitted methods.
pub fn prepare_generation_ir(
    ir: &CoreIr,
    common: &crate::CommonConfig,
    target: ModelTarget,
    identifier: fn(&str) -> String,
) -> Result<CoreIr> {
    let mut prepared = prepare_response_models(ir, &common.models, target, identifier)?;
    if common.skip_deprecated_operations {
        prepared
            .operations
            .retain(|op| op.attributes.get("deprecated").and_then(Value::as_bool) != Some(true));
    }
    Ok(prepared)
}

#[cfg(test)]
mod tests;
