use std::collections::BTreeSet;

use anyhow::Result;
use indexmap::IndexMap;
use serde_json::Value;

use crate::diagnostic::DiagnosticKind;
use crate::document::{Schema, SchemaOrBool, SchemaTypeDecl};
use arvalez_ir::TypeRef;

// Forward reference - merge functions are called by OpenApiImporter.merge_schemas
// so they accept a mutable reference to the importer.
use crate::importer::OpenApiImporter;


pub(crate) fn dedupe_variants(variants: Vec<TypeRef>) -> Vec<TypeRef> {
    let mut seen = BTreeSet::new();
    let mut deduped = Vec::new();
    for variant in variants {
        let key = serde_json::to_string(&variant).expect("type refs should always serialize");
        if seen.insert(key) {
            deduped.push(variant);
        }
    }
    deduped
}

pub(crate) fn merge_required(mut left: Vec<String>, right: Vec<String>) -> Vec<String> {
    let mut seen = left.iter().cloned().collect::<BTreeSet<_>>();
    for value in right {
        if seen.insert(value.clone()) {
            left.push(value);
        }
    }
    left
}

pub(crate) fn merge_optional_field<T>(
    target: &mut Option<T>,
    incoming: Option<T>,
    field_name: &str,
    context: &str,
    importer: &mut OpenApiImporter,
) -> Result<()>
where
    T: PartialEq,
{
    match (target.as_ref(), incoming) {
        (_, None) => {}
        (None, Some(value)) => *target = Some(value),
        (Some(existing), Some(value)) if *existing == value => {}
        (Some(_), Some(_)) => {
            importer.handle_unhandled(
                context,
                DiagnosticKind::IncompatibleAllOfField {
                    field: field_name.to_owned(),
                },
            )?;
        }
    }
    Ok(())
}

pub(crate) fn merge_non_codegen_optional_field<T>(target: &mut Option<T>, incoming: Option<T>) {
    if target.is_none() {
        *target = incoming;
    }
}

pub(crate) fn merge_schema_types(
    inferred_left: Option<SchemaTypeDecl>,
    inferred_right: Option<SchemaTypeDecl>,
    left_is_generic_object_placeholder: bool,
    right_is_generic_object_placeholder: bool,
    left: Option<SchemaTypeDecl>,
    right: Option<SchemaTypeDecl>,
    _context: &str,
    _importer: &mut OpenApiImporter,
) -> Result<Option<SchemaTypeDecl>> {
    match (left, right) {
        (None, None) => Ok(inferred_left.or(inferred_right)),
        (Some(value), None) => Ok(Some(value)),
        (None, Some(value)) => Ok(Some(value)),
        (Some(left), Some(right)) if left == right => Ok(Some(left)),
        (Some(left), Some(right)) => {
            let left_inferred = inferred_left.unwrap_or(left.clone());
            let right_inferred = inferred_right.unwrap_or(right.clone());
            if left_is_generic_object_placeholder {
                return Ok(Some(right_inferred));
            }
            if right_is_generic_object_placeholder {
                return Ok(Some(left_inferred));
            }
            if let Some(merged) =
                merge_numeric_compatible_schema_types(&left_inferred, &right_inferred)
            {
                return Ok(Some(merged));
            }
            if let Some(merged) =
                merge_nullable_compatible_schema_types(&left_inferred, &right_inferred)
            {
                return Ok(Some(merged));
            }
            if left_inferred == right_inferred {
                Ok(Some(left_inferred))
            } else {
                // Incompatible types in allOf: keep the left (base) type and continue.
                Ok(Some(left_inferred))
            }
        }
    }
}

pub(crate) fn merge_numeric_compatible_schema_types(
    left: &SchemaTypeDecl,
    right: &SchemaTypeDecl,
) -> Option<SchemaTypeDecl> {
    let left_variants = left.as_slice();
    let right_variants = right.as_slice();
    let left_has_numeric = left_variants
        .iter()
        .any(|value| value == "integer" || value == "number");
    let right_has_numeric = right_variants
        .iter()
        .any(|value| value == "integer" || value == "number");
    if !left_has_numeric || !right_has_numeric {
        return None;
    }

    let left_other = left_variants
        .iter()
        .filter(|value| value.as_str() != "integer" && value.as_str() != "number")
        .collect::<BTreeSet<_>>();
    let right_other = right_variants
        .iter()
        .filter(|value| value.as_str() != "integer" && value.as_str() != "number")
        .collect::<BTreeSet<_>>();
    if left_other != right_other {
        return None;
    }

    let mut merged = left_other
        .into_iter()
        .map(|value| value.to_owned())
        .collect::<Vec<_>>();
    merged.push("number".into());

    Some(if merged.len() == 1 {
        SchemaTypeDecl::Single(merged.remove(0))
    } else {
        SchemaTypeDecl::Multiple(merged)
    })
}

pub(crate) fn merge_nullable_compatible_schema_types(
    left: &SchemaTypeDecl,
    right: &SchemaTypeDecl,
) -> Option<SchemaTypeDecl> {
    let left_variants = left.as_slice();
    let right_variants = right.as_slice();
    if left_variants.is_empty() || right_variants.is_empty() {
        return None;
    }

    let left_has_null = left_variants.iter().any(|value| value == "null");
    let right_has_null = right_variants.iter().any(|value| value == "null");
    if !left_has_null && !right_has_null {
        return None;
    }

    let left_without_null = left_variants
        .iter()
        .filter(|value| value.as_str() != "null")
        .cloned()
        .collect::<BTreeSet<_>>();
    let right_without_null = right_variants
        .iter()
        .filter(|value| value.as_str() != "null")
        .cloned()
        .collect::<BTreeSet<_>>();

    let merged_without_null = if left_without_null.is_empty() && !right_without_null.is_empty() {
        right_without_null
    } else if right_without_null.is_empty() && !left_without_null.is_empty() {
        left_without_null
    } else if left_without_null == right_without_null {
        left_without_null
    } else {
        return None;
    };

    let mut merged = merged_without_null.into_iter().collect::<Vec<_>>();
    merged.push("null".into());

    Some(if merged.len() == 1 {
        SchemaTypeDecl::Single(merged.remove(0))
    } else {
        SchemaTypeDecl::Multiple(merged)
    })
}

pub(crate) fn merge_enum_values(
    left: Option<Vec<Value>>,
    right: Option<Vec<Value>>,
    _context: &str,
    _importer: &mut OpenApiImporter,
) -> Result<Option<Vec<Value>>> {
    match (left, right) {
        (None, None) => Ok(None),
        (Some(values), None) | (None, Some(values)) => Ok(Some(values)),
        (Some(left_values), Some(right_values)) => {
            let right_keys = right_values
                .iter()
                .map(serde_json::to_string)
                .collect::<std::result::Result<BTreeSet<_>, _>>()
                .expect("enum values should always serialize");
            let merged = left_values
                .iter()
                .filter(|value| {
                    let key =
                        serde_json::to_string(value).expect("enum values should always serialize");
                    right_keys.contains(&key)
                })
                .cloned()
                .collect::<Vec<_>>();

            // If the intersection is empty the enum sets are disjoint.
            // Accept all values from the left side as a graceful fallback.
            let result = if merged.is_empty() {
                left_values
            } else {
                merged
            };

            Ok(Some(result))
        }
    }
}

pub(crate) fn infer_schema_type_for_merge(schema: &Schema) -> Option<SchemaTypeDecl> {
    schema.schema_type.clone().or_else(|| {
        if schema.properties.is_some() || schema.additional_properties.is_some() {
            Some(SchemaTypeDecl::Single("object".into()))
        } else if schema.items.is_some() {
            Some(SchemaTypeDecl::Single("array".into()))
        } else if let Some(enum_values) = &schema.enum_values {
            match infer_enum_type(enum_values, schema.format.as_deref()) {
                TypeRef::Primitive { name } => Some(SchemaTypeDecl::Single(name)),
                _ => None,
            }
        } else {
            infer_format_only_type(schema.format.as_deref()).and_then(|type_ref| match type_ref {
                TypeRef::Primitive { name } => Some(SchemaTypeDecl::Single(name)),
                _ => None,
            })
        }
    })
}

pub(crate) fn infer_enum_type(enum_values: &[Value], format: Option<&str>) -> TypeRef {
    let inferred_name = if enum_values.iter().all(Value::is_string) {
        if format == Some("binary") {
            "binary"
        } else {
            "string"
        }
    } else if enum_values.iter().all(|value| value.as_i64().is_some()) {
        "integer"
    } else if enum_values.iter().all(Value::is_number) {
        "number"
    } else if enum_values.iter().all(Value::is_boolean) {
        "boolean"
    } else {
        "any"
    };

    TypeRef::primitive(inferred_name)
}

pub(crate) fn infer_format_only_type(format: Option<&str>) -> Option<TypeRef> {
    let inferred = match format? {
        "binary" => "binary",
        // Allow the primitive type names themselves used as format values.
        "boolean" | "bool" => "boolean",
        "integer" | "int" | "int32" | "int64" => "integer",
        "number" | "float" | "double" | "decimal" => "number",
        // "string" (and related) as format → infer string type.
        "string" | "byte" | "date" | "date-time" | "duration" | "email" | "hostname"
        | "host-name" | "ipv4" | "ipv6" | "password" | "uri" | "uuid" => "string",
        _ => return None,
    };
    Some(TypeRef::primitive(inferred))
}

pub(crate) fn merge_properties(
    importer: &mut OpenApiImporter,
    mut left: IndexMap<String, SchemaOrBool>,
    right: IndexMap<String, SchemaOrBool>,
    context: &str,
) -> Result<IndexMap<String, SchemaOrBool>> {
    for (key, value) in right {
        if let Some(existing) = left.shift_remove(&key) {
            let merged = match (existing, value) {
                (SchemaOrBool::Schema(l), SchemaOrBool::Schema(r)) => {
                    SchemaOrBool::Schema(importer.merge_schemas(l, r, context)?)
                }
                // Boolean schema vs real schema: prefer the real schema.
                (SchemaOrBool::Schema(s), SchemaOrBool::Bool(_))
                | (SchemaOrBool::Bool(_), SchemaOrBool::Schema(s)) => SchemaOrBool::Schema(s),
                // Both boolean: keep default.
                (SchemaOrBool::Bool(_), SchemaOrBool::Bool(_)) => SchemaOrBool::default(),
            };
            left.insert(key, merged);
        } else {
            left.insert(key, value);
        }
    }
    Ok(left)
}