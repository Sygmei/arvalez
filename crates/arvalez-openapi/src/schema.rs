use anyhow::{Result, anyhow, bail};
use arvalez_ir::{Attributes, TypeRef};
use serde_json::Value;

use crate::document::{
    AdditionalProperties, MediaTypeSpec, ParameterSpec, ResponseSpec, Schema, SchemaOrBool,
    SchemaTypeDecl,
};


pub(crate) fn ref_name(reference: &str) -> Result<String> {
    reference
        .rsplit('/')
        .next()
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| anyhow!("unsupported reference `{reference}`"))
}

pub(crate) fn is_named_schema_reference(reference: &str) -> bool {
    let Some(pointer) = reference.strip_prefix("#/") else {
        return false;
    };
    let segments = pointer.split('/').collect::<Vec<_>>();
    matches!(
        segments.as_slice(),
        ["components", "schemas", _] | ["definitions", _]
    )
}

pub(crate) fn is_inline_local_schema_reference(reference: &str) -> bool {
    reference.starts_with("#/") && !is_named_schema_reference(reference)
}

pub(crate) fn decode_json_pointer_segment(segment: &str) -> Result<String> {
    let unescaped = segment.replace("~1", "/").replace("~0", "~");
    percent_decode(&unescaped)
}

pub(crate) fn percent_decode(value: &str) -> Result<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() {
                bail!("unsupported reference segment `{value}`");
            }
            let high = (bytes[index + 1] as char)
                .to_digit(16)
                .ok_or_else(|| anyhow!("unsupported reference segment `{value}`"))?;
            let low = (bytes[index + 2] as char)
                .to_digit(16)
                .ok_or_else(|| anyhow!("unsupported reference segment `{value}`"))?;
            decoded.push(((high << 4) | low) as u8);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }

    String::from_utf8(decoded).map_err(|_| anyhow!("unsupported reference segment `{value}`"))
}

/// Resolve a `$ref` that points into a `ResponseSpec`, optionally continuing
/// into `content/{media_type}/schema/...`.
pub(crate) fn resolve_response_schema_reference(
    response: &ResponseSpec,
    segments: &[String],
    reference: &str,
) -> Result<Schema> {
    match segments {
        // Referencing the response object itself — use its primary schema.
        // If the response has no content (e.g. a description-only response used
        // mistakenly as a schema $ref), return an empty schema so callers treat
        // this as `any` rather than failing.
        [] => {
            let schema = response
                .content
                .values()
                .find_map(|media| media.schema.as_ref())
                .cloned()
                .unwrap_or_default();
            Ok(schema)
        }
        // content/{media_type}/schema/...
        [content_key, media_type, schema_key, rest @ ..]
            if content_key == "content" && schema_key == "schema" =>
        {
            let schema = response
                .content
                .get(media_type)
                .and_then(|media| media.schema.as_ref())
                .ok_or_else(|| anyhow!("unsupported reference `{reference}`"))?;
            resolve_nested_schema_reference(schema, rest, reference)
        }
        _ => Err(anyhow!("unsupported reference `{reference}`")),
    }
}

pub(crate) fn resolve_nested_schema_reference(
    schema: &Schema,
    segments: &[String],
    reference: &str,
) -> Result<Schema> {
    if segments.is_empty() {
        return Ok(schema.clone());
    }

    match segments {
        [segment, name, remainder @ ..] if segment == "definitions" => {
            let nested = schema
                .definitions
                .as_ref()
                .and_then(|definitions| definitions.get(name))
                .ok_or_else(|| anyhow!("unsupported reference `{reference}`"))?;
            resolve_nested_schema_reference(nested, remainder, reference)
        }
        [segment, remainder @ ..] if segment == "allOf" => {
            let index = remainder
                .first()
                .and_then(|value| value.parse::<usize>().ok())
                .ok_or_else(|| anyhow!("unsupported reference `{reference}`"))?;
            let member = schema
                .all_of
                .as_ref()
                .and_then(|members| members.get(index))
                .ok_or_else(|| anyhow!("unsupported reference `{reference}`"))?;
            resolve_nested_schema_reference(member, &remainder[1..], reference)
        }
        [segment, remainder @ ..] if segment == "anyOf" => {
            let index = remainder
                .first()
                .and_then(|value| value.parse::<usize>().ok())
                .ok_or_else(|| anyhow!("unsupported reference `{reference}`"))?;
            let member = schema
                .any_of
                .as_ref()
                .and_then(|members| members.get(index))
                .ok_or_else(|| anyhow!("unsupported reference `{reference}`"))?;
            resolve_nested_schema_reference(member, &remainder[1..], reference)
        }
        [segment, remainder @ ..] if segment == "oneOf" => {
            let index = remainder
                .first()
                .and_then(|value| value.parse::<usize>().ok())
                .ok_or_else(|| anyhow!("unsupported reference `{reference}`"))?;
            let member = schema
                .one_of
                .as_ref()
                .and_then(|members| members.get(index))
                .ok_or_else(|| anyhow!("unsupported reference `{reference}`"))?;
            resolve_nested_schema_reference(member, &remainder[1..], reference)
        }
        [segment, name, remainder @ ..] if segment == "properties" => {
            // Try top-level properties first.
            if let Some(property) = schema
                .properties
                .as_ref()
                .and_then(|p| p.get(name))
                .and_then(SchemaOrBool::as_schema)
            {
                return resolve_nested_schema_reference(property, remainder, reference);
            }
            // If the schema uses allOf with no top-level properties (e.g. a schema
            // whose properties are spread across its allOf members), search members.
            if let Some(all_of) = &schema.all_of {
                for member in all_of {
                    if let Some(property) = member
                        .properties
                        .as_ref()
                        .and_then(|p| p.get(name))
                        .and_then(SchemaOrBool::as_schema)
                    {
                        return resolve_nested_schema_reference(property, remainder, reference);
                    }
                }
            }
            Err(anyhow!("unsupported reference `{reference}`"))
        }
        [segment, remainder @ ..] if segment == "items" => {
            let item = schema
                .items
                .as_deref()
                .ok_or_else(|| anyhow!("unsupported reference `{reference}`"))?;
            resolve_nested_schema_reference(item, remainder, reference)
        }
        [segment, remainder @ ..] if segment == "additionalProperties" => {
            let nested = match schema.additional_properties.as_ref() {
                Some(AdditionalProperties::Schema(schema)) => schema.as_ref(),
                _ => return Err(anyhow!("unsupported reference `{reference}`")),
            };
            resolve_nested_schema_reference(nested, remainder, reference)
        }
        _ => Err(anyhow!("unsupported reference `{reference}`")),
    }
}

pub(crate) fn schema_is_object_like(schema: &Schema) -> bool {
    schema
        .schema_type_variants()
        .is_some_and(|variants| variants.iter().any(|value| value == "object"))
        || schema.properties.is_some()
        || schema.additional_properties.is_some()
}

pub(crate) fn is_validation_only_schema_variant(schema: &Schema) -> bool {
    schema.reference.is_none()
        && schema.definitions.is_none()
        && schema
            .schema_type
            .as_ref()
            .is_none_or(|decl| matches!(decl.as_slice(), [value] if value == "object"))
        && schema.format.is_none()
        && schema.const_value.is_none()
        && schema._discriminator.is_none()
        && schema.all_of.is_none()
        && schema.enum_values.is_none()
        && schema.properties.is_none()
        && schema.items.is_none()
        && schema.additional_properties.is_none()
        && schema.any_of.is_none()
        && schema.one_of.is_none()
        && schema
            .extra_keywords
            .keys()
            .all(|keyword| is_known_ignored_schema_keyword(keyword) || keyword.starts_with("x-"))
}

pub(crate) fn is_generic_object_placeholder(schema: &Schema) -> bool {
    let has_object_type = schema
        .schema_type
        .as_ref()
        .is_some_and(|decl| matches!(decl.as_slice(), [value] if value == "object"));

    (has_object_type || schema.properties.is_some())
        && schema
            .properties
            .as_ref()
            .is_some_and(|properties| properties.is_empty())
        && schema.additional_properties.is_none()
        && schema.definitions.is_none()
        && schema.items.is_none()
        && schema.enum_values.is_none()
        && schema.const_value.is_none()
        && schema.any_of.is_none()
        && schema.one_of.is_none()
        && schema.all_of.is_none()
        && schema._discriminator.is_none()
}

pub(crate) fn schema_runtime_attributes(schema: &Schema) -> Attributes {
    let mut attributes = Attributes::default();
    if let Some(description) = schema
        .extra_keywords
        .get("description")
        .and_then(Value::as_str)
    {
        attributes.insert("description".into(), Value::String(description.to_owned()));
    }
    if let Some(content_encoding) = schema
        .extra_keywords
        .get("contentEncoding")
        .and_then(Value::as_str)
    {
        attributes.insert(
            "content_encoding".into(),
            Value::String(content_encoding.to_owned()),
        );
    }
    if let Some(content_media_type) = schema
        .extra_keywords
        .get("contentMediaType")
        .and_then(Value::as_str)
    {
        attributes.insert(
            "content_media_type".into(),
            Value::String(content_media_type.to_owned()),
        );
    }
    attributes
}

pub(crate) fn parameter_attributes(param: &ParameterSpec, schema: &Schema) -> Attributes {
    let mut attributes = schema_runtime_attributes(schema);
    if !param.description.trim().is_empty() {
        attributes.insert(
            "description".into(),
            Value::String(param.description.trim().to_owned()),
        );
    }
    if let Some(collection_format) = &param.collection_format {
        attributes.insert(
            "collection_format".into(),
            Value::String(collection_format.clone()),
        );
    }
    attributes
}

pub(crate) fn is_unconstrained_schema(schema: &Schema) -> bool {
    schema.reference.is_none()
        && schema.definitions.is_none()
        && schema.schema_type.is_none()
        && schema.format.is_none()
        && schema.const_value.is_none()
        && schema._discriminator.is_none()
        && schema.all_of.is_none()
        && schema.enum_values.is_none()
        && schema.properties.is_none()
        && schema.required.is_none()
        && schema.items.is_none()
        && schema.additional_properties.is_none()
        && schema.any_of.is_none()
        && schema.one_of.is_none()
        && schema
            .extra_keywords
            .keys()
            .all(|keyword| is_known_ignored_schema_keyword(keyword) || keyword.starts_with("x-"))
}

pub(crate) fn schema_has_non_all_of_shape(schema: &Schema) -> bool {
    schema.reference.is_some()
        || schema.definitions.is_some()
        || schema.schema_type.is_some()
        || schema.format.is_some()
        || schema.const_value.is_some()
        || schema.enum_values.is_some()
        || schema.properties.is_some()
        || schema.required.is_some()
        || schema.items.is_some()
        || schema.additional_properties.is_some()
        || schema.any_of.is_some()
        || schema.one_of.is_some()
        || schema._discriminator.is_some()
}

pub(crate) fn is_known_ignored_schema_keyword(keyword: &str) -> bool {
    matches!(
        keyword,
        "default"
            | "not"
            | "description"
            | "example"
            | "examples"
            | "collectionFormat"
            | "contentEncoding"
            | "contentMediaType"
            | "externalDocs"
            | "xml"
            | "deprecated"
            | "readOnly"
            | "writeOnly"
            | "minimum"
            | "maximum"
            | "exclusiveMinimum"
            | "exclusiveMaximum"
            | "multipleOf"
            | "minLength"
            | "maxLength"
            | "pattern"
            | "minItems"
            | "maxItems"
            | "uniqueItems"
            | "minProperties"
            | "maxProperties"
            | "nullable"
            | "$schema"
            | "$id"
            | "$comment"
    )
}

pub(crate) fn is_known_but_unimplemented_schema_keyword(keyword: &str) -> bool {
    matches!(
        keyword,
        "if" | "then"
            | "else"
            | "contains"
            | "prefixItems"
            | "patternProperties"
            | "propertyNames"
            | "dependentSchemas"
            | "unevaluatedProperties"
            | "unevaluatedItems"
            | "$defs"
    )
}