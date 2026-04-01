use std::collections::BTreeMap;

use indexmap::IndexMap;
use serde::Deserialize;
use serde_json::Value;

use arvalez_ir::ParameterLocation;


fn deserialize_paths_map<'de, D>(
    deserializer: D,
) -> std::result::Result<BTreeMap<String, PathItem>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    // Use a proper Visitor so the `serde_path_to_error`-wrapped deserializer
    // stays in scope when each PathItem is deserialized.  Deserializing via an
    // intermediate `BTreeMap<String, Value>` then `serde_json::from_value` would
    // spin up a fresh deserialization context, losing all path tracking and
    // causing errors to be reported as just `paths` instead of the full path.
    struct PathsMapVisitor;

    impl<'de> serde::de::Visitor<'de> for PathsMapVisitor {
        type Value = BTreeMap<String, PathItem>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a map of path items")
        }

        fn visit_map<A>(self, mut map: A) -> std::result::Result<Self::Value, A::Error>
        where
            A: serde::de::MapAccess<'de>,
        {
            let mut result = BTreeMap::new();
            while let Some(key) = map.next_key::<String>()? {
                if key.starts_with("x-") {
                    // Drain the value without deserializing it.
                    map.next_value::<serde::de::IgnoredAny>()?;
                } else {
                    // Deserialize directly as PathItem so that serde_path_to_error
                    // can track the path key → PathItem fields.
                    let value = map.next_value::<PathItem>()?;
                    result.insert(key, value);
                }
            }
            Ok(result)
        }
    }

    deserializer.deserialize_map(PathsMapVisitor)
}



/// Version-specific input struct for Swagger 2.0 documents.
/// Top-level `definitions`, `parameters`, `responses`, and `consumes` live here;
/// OpenAPI 3's `components` is absent.
#[derive(Debug, Deserialize, Clone)]
pub(crate) struct Swagger2Document {
    #[serde(default)]
    #[serde(deserialize_with = "deserialize_paths_map")]
    pub(crate) paths: BTreeMap<String, PathItem>,
    #[serde(default)]
    pub(crate) consumes: Vec<String>,
    #[serde(default)]
    pub(crate) parameters: BTreeMap<String, ParameterSpec>,
    #[serde(rename = "definitions")]
    #[serde(default)]
    pub(crate) definitions: BTreeMap<String, Schema>,
    #[serde(default)]
    pub(crate) responses: BTreeMap<String, ResponseSpec>,
}

/// Version-specific input struct for OpenAPI 3.x documents.
/// Top-level `definitions`, `consumes`, etc. are absent; everything lives under `components`.
#[derive(Debug, Deserialize, Clone)]
pub(crate) struct OpenApi3Document {
    #[serde(default)]
    #[serde(deserialize_with = "deserialize_paths_map")]
    pub(crate) paths: BTreeMap<String, PathItem>,
    #[serde(default)]
    pub(crate) components: Components,
}

/// Normalised internal document form fed to `OpenApiImporter`.
/// Retains `Deserialize` so unit-test helpers can construct it directly from inline JSON fixtures.
#[derive(Debug, Deserialize, Clone)]
pub(crate) struct OpenApiDocument {
    #[serde(default)]
    #[serde(deserialize_with = "deserialize_paths_map")]
    pub(crate) paths: BTreeMap<String, PathItem>,
    #[serde(default)]
    pub(crate) consumes: Vec<String>,
    #[serde(default)]
    pub(crate) parameters: BTreeMap<String, ParameterSpec>,
    #[serde(rename = "definitions")]
    #[serde(default)]
    pub(crate) definitions: BTreeMap<String, Schema>,
    #[serde(default)]
    pub(crate) responses: BTreeMap<String, ResponseSpec>,
    #[serde(default)]
    pub(crate) components: Components,
}

impl From<Swagger2Document> for OpenApiDocument {
    fn from(doc: Swagger2Document) -> Self {
        Self {
            paths: doc.paths,
            consumes: doc.consumes,
            parameters: doc.parameters,
            definitions: doc.definitions,
            responses: doc.responses,
            components: Components::default(),
        }
    }
}

impl From<OpenApi3Document> for OpenApiDocument {
    fn from(doc: OpenApi3Document) -> Self {
        Self {
            paths: doc.paths,
            consumes: Vec::new(),
            parameters: BTreeMap::new(),
            definitions: BTreeMap::new(),
            responses: BTreeMap::new(),
            components: doc.components,
        }
    }
}

#[derive(Debug, Deserialize, Default, Clone)]
pub(crate) struct Components {
    #[serde(default)]
    pub(crate) schemas: BTreeMap<String, Schema>,
    #[serde(default)]
    pub(crate) parameters: BTreeMap<String, ParameterSpec>,
    #[serde(rename = "requestBodies")]
    #[serde(default)]
    pub(crate) request_bodies: BTreeMap<String, RequestBodyOrRef>,
    #[serde(default)]
    pub(crate) responses: BTreeMap<String, ResponseSpec>,
}

#[derive(Debug, Deserialize, Default, Clone)]
pub(crate) struct PathItem {
    #[serde(default)]
    pub(crate) parameters: Option<Vec<ParameterOrRef>>,
    #[serde(default)]
    pub(crate) get: Option<OperationSpec>,
    #[serde(default)]
    pub(crate) post: Option<OperationSpec>,
    #[serde(default)]
    pub(crate) put: Option<OperationSpec>,
    #[serde(default)]
    pub(crate) patch: Option<OperationSpec>,
    #[serde(default)]
    pub(crate) delete: Option<OperationSpec>,
}

#[derive(Debug, Deserialize, Default, Clone)]
pub(crate) struct OperationSpec {
    #[serde(rename = "operationId")]
    #[serde(default)]
    pub(crate) operation_id: Option<String>,
    #[serde(default)]
    pub(crate) summary: Option<String>,
    #[serde(default)]
    pub(crate) tags: Vec<String>,
    #[serde(default)]
    pub(crate) parameters: Vec<ParameterOrRef>,
    #[serde(default)]
    pub(crate) consumes: Vec<String>,
    #[serde(rename = "requestBody")]
    #[serde(default)]
    pub(crate) request_body: Option<RequestBodyOrRef>,
    #[serde(default)]
    pub(crate) responses: BTreeMap<String, ResponseSpecOrRef>,
}

#[derive(Debug, Deserialize, Clone)]
pub(crate) struct ParameterSpec {
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) description: String,
    #[serde(rename = "in")]
    pub(crate) location: RawParameterLocation,
    #[serde(default)]
    pub(crate) required: bool,
    #[serde(default)]
    pub(crate) schema: Option<Schema>,
    #[serde(rename = "type")]
    #[serde(default)]
    pub(crate) parameter_type: Option<SchemaTypeDecl>,
    #[serde(default)]
    pub(crate) format: Option<String>,
    #[serde(default)]
    pub(crate) items: Option<Box<Schema>>,
    #[serde(rename = "collectionFormat")]
    #[serde(default)]
    pub(crate) collection_format: Option<String>,
    /// OpenAPI 3 alternative to `schema`: a single-entry media-type map.
    #[serde(default)]
    pub(crate) content: BTreeMap<String, MediaTypeSpec>,
}

impl ParameterSpec {
    pub(crate) fn effective_schema(&self) -> Option<Schema> {
        self.schema
            .clone()
            .or_else(|| {
                self.parameter_type.clone().map(|schema_type| Schema {
                    schema_type: Some(schema_type),
                    format: self.format.clone(),
                    items: self.items.clone(),
                    ..Schema::default()
                })
            })
            .or_else(|| {
                // OpenAPI 3 allows `content` instead of `schema` on parameters.
                // Use the schema from the first (and per-spec, only) entry.
                self.content
                    .values()
                    .next()
                    .and_then(|media| media.schema.clone())
            })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RawParameterLocation {
    Path,
    Query,
    Header,
    Cookie,
    Body,
    FormData,
}

impl RawParameterLocation {
    pub(crate) fn as_ir_location(self) -> Option<ParameterLocation> {
        match self {
            Self::Path => Some(ParameterLocation::Path),
            Self::Query => Some(ParameterLocation::Query),
            Self::Header => Some(ParameterLocation::Header),
            Self::Cookie => Some(ParameterLocation::Cookie),
            Self::Body | Self::FormData => None,
        }
    }
}

impl<'de> Deserialize<'de> for RawParameterLocation {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        match value.as_str() {
            "path" => Ok(Self::Path),
            "query" => Ok(Self::Query),
            "header" => Ok(Self::Header),
            "cookie" => Ok(Self::Cookie),
            "body" => Ok(Self::Body),
            "formData" | "formdata" => Ok(Self::FormData),
            _ => Err(serde::de::Error::unknown_variant(
                &value,
                &["path", "query", "header", "cookie", "body", "formData"],
            )),
        }
    }
}

pub(crate) fn raw_parameter_location_label(location: RawParameterLocation) -> &'static str {
    match location {
        RawParameterLocation::Path => "path",
        RawParameterLocation::Query => "query",
        RawParameterLocation::Header => "header",
        RawParameterLocation::Cookie => "cookie",
        RawParameterLocation::Body => "body",
        RawParameterLocation::FormData => "form_data",
    }
}

#[derive(Debug, Deserialize, Clone)]
#[serde(untagged)]
pub(crate) enum ParameterOrRef {
    Ref {
        #[serde(rename = "$ref")]
        reference: String,
    },
    Inline(ParameterSpec),
}

#[derive(Debug, Deserialize, Default, Clone)]
pub(crate) struct RequestBodySpec {
    #[serde(default)]
    pub(crate) required: bool,
    #[serde(default)]
    pub(crate) content: BTreeMap<String, MediaTypeSpec>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(untagged)]
pub(crate) enum RequestBodyOrRef {
    Ref {
        #[serde(rename = "$ref")]
        reference: String,
    },
    Inline(RequestBodySpec),
}

/// A response entry that may be either an inline spec or a `$ref` pointer.
/// Using a flat struct (rather than an untagged enum) ensures that
/// `serde_path_to_error` can track the full JSON/YAML path through the
/// struct's fields, giving accurate error locations on parse failure.
#[derive(Debug, Deserialize, Default, Clone)]
pub(crate) struct ResponseSpecOrRef {
    #[serde(rename = "$ref")]
    #[serde(default)]
    pub(crate) reference: Option<String>,
    #[serde(default)]
    pub(crate) description: String,
    #[serde(default)]
    pub(crate) content: BTreeMap<String, MediaTypeSpec>,
}

#[derive(Debug, Deserialize, Default, Clone)]
pub(crate) struct ResponseSpec {
    #[serde(default)]
    pub(crate) description: String,
    #[serde(default)]
    pub(crate) content: BTreeMap<String, MediaTypeSpec>,
}

#[derive(Debug, Deserialize, Default, Clone)]
pub(crate) struct MediaTypeSpec {
    #[serde(default)]
    pub(crate) schema: Option<Schema>,
}

#[derive(Debug, Deserialize, Default, Clone, PartialEq)]
pub(crate) struct Schema {
    #[serde(rename = "$ref")]
    #[serde(default)]
    pub(crate) reference: Option<String>,
    #[serde(default)]
    pub(crate) definitions: Option<BTreeMap<String, Schema>>,
    #[serde(rename = "type")]
    #[serde(default)]
    pub(crate) schema_type: Option<SchemaTypeDecl>,
    #[serde(default)]
    pub(crate) title: Option<String>,
    #[serde(default)]
    pub(crate) format: Option<String>,
    #[serde(rename = "const")]
    #[serde(default)]
    pub(crate) const_value: Option<Value>,
    #[serde(rename = "discriminator")]
    #[serde(default)]
    pub(crate) _discriminator: Option<Value>,
    #[serde(rename = "allOf")]
    #[serde(default)]
    pub(crate) all_of: Option<Vec<Schema>>,
    #[serde(rename = "enum")]
    #[serde(default)]
    pub(crate) enum_values: Option<Vec<Value>>,
    #[serde(default)]
    pub(crate) properties: Option<IndexMap<String, SchemaOrBool>>,
    #[serde(default)]
    pub(crate) required: Option<Vec<String>>,
    #[serde(default)]
    pub(crate) items: Option<Box<Schema>>,
    #[serde(rename = "additionalProperties")]
    #[serde(default)]
    pub(crate) additional_properties: Option<AdditionalProperties>,
    #[serde(rename = "anyOf")]
    #[serde(default)]
    pub(crate) any_of: Option<Vec<Schema>>,
    #[serde(rename = "oneOf")]
    #[serde(default)]
    pub(crate) one_of: Option<Vec<Schema>>,
    // Capture numeric constraint keywords explicitly to avoid serde_yaml integer
    // coercion failures that occur when these pass through the flattened map.
    #[serde(default)]
    pub(crate) minimum: Option<Value>,
    #[serde(default)]
    pub(crate) maximum: Option<Value>,
    #[serde(rename = "exclusiveMinimum")]
    #[serde(default)]
    pub(crate) exclusive_minimum: Option<Value>,
    #[serde(rename = "exclusiveMaximum")]
    #[serde(default)]
    pub(crate) exclusive_maximum: Option<Value>,
    #[serde(default)]
    #[serde(rename = "multipleOf")]
    pub(crate) multiple_of: Option<Value>,
    #[serde(default)]
    #[serde(rename = "minLength")]
    pub(crate) min_length: Option<Value>,
    #[serde(default)]
    #[serde(rename = "maxLength")]
    pub(crate) max_length: Option<Value>,
    #[serde(default)]
    #[serde(rename = "minItems")]
    pub(crate) min_items: Option<Value>,
    #[serde(default)]
    #[serde(rename = "maxItems")]
    pub(crate) max_items: Option<Value>,
    #[serde(default)]
    #[serde(rename = "minProperties")]
    pub(crate) min_properties: Option<Value>,
    #[serde(default)]
    #[serde(rename = "maxProperties")]
    pub(crate) max_properties: Option<Value>,
    #[serde(flatten)]
    #[serde(default)]
    pub(crate) extra_keywords: BTreeMap<String, Value>,
}

#[derive(Debug, Deserialize, Clone, PartialEq)]
#[serde(untagged)]
pub(crate) enum AdditionalProperties {
    Bool(bool),
    Schema(Box<Schema>),
}

/// A property schema that may be a full schema object or a boolean schema
/// (valid in OpenAPI 3.1 / JSON Schema: `false` = never valid, `true` = always valid).
/// Boolean schemas are treated as absent properties for code-generation purposes.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SchemaOrBool {
    Schema(Schema),
    Bool(bool),
}

impl Default for SchemaOrBool {
    fn default() -> Self {
        SchemaOrBool::Schema(Schema::default())
    }
}

impl SchemaOrBool {
    /// Returns the inner schema, or `None` for boolean schemas.
    pub(crate) fn as_schema(&self) -> Option<&Schema> {
        match self {
            SchemaOrBool::Schema(s) => Some(s),
            SchemaOrBool::Bool(_) => None,
        }
    }
}

impl<'de> serde::Deserialize<'de> for SchemaOrBool {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct SchemaOrBoolVisitor;
        impl<'de> serde::de::Visitor<'de> for SchemaOrBoolVisitor {
            type Value = SchemaOrBool;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "a JSON Schema object or boolean")
            }
            // Boolean schemas: `false` = never valid, `true` = always valid.
            fn visit_bool<E: serde::de::Error>(self, v: bool) -> std::result::Result<SchemaOrBool, E> {
                Ok(SchemaOrBool::Bool(v))
            }
            // Map: deserialize as a full Schema.  Using MapAccessDeserializer keeps
            // the serde_path_to_error-wrapped MapAccess in play so field-level
            // errors within the schema are tracked correctly.
            fn visit_map<A: serde::de::MapAccess<'de>>(self, map: A) -> std::result::Result<SchemaOrBool, A::Error> {
                let schema = Schema::deserialize(serde::de::value::MapAccessDeserializer::new(map))?;
                Ok(SchemaOrBool::Schema(schema))
            }
        }
        deserializer.deserialize_any(SchemaOrBoolVisitor)
    }
}

#[derive(Debug, Deserialize, Clone, PartialEq)]
#[serde(untagged)]
pub(crate) enum SchemaTypeDecl {
    Single(String),
    Multiple(Vec<String>),
    Embedded(Box<Schema>),
}

impl SchemaTypeDecl {
    pub(crate) fn as_slice(&self) -> &[String] {
        match self {
            Self::Single(value) => std::slice::from_ref(value),
            Self::Multiple(values) => values.as_slice(),
            Self::Embedded(_) => &[],
        }
    }

    pub(crate) fn embedded_schema(&self) -> Option<&Schema> {
        match self {
            Self::Embedded(schema) => Some(schema.as_ref()),
            _ => None,
        }
    }
}

impl Schema {
    pub(crate) fn schema_type_variants(&self) -> Option<&[String]> {
        self.schema_type.as_ref().map(SchemaTypeDecl::as_slice)
    }

    pub(crate) fn primary_schema_type(&self) -> Option<&str> {
        self.schema_type_variants()?
            .iter()
            .find(|value| value.as_str() != "null")
            .map(String::as_str)
    }

    pub(crate) fn is_exact_null_type(&self) -> bool {
        matches!(self.schema_type_variants(), Some([value]) if value == "null")
    }
}