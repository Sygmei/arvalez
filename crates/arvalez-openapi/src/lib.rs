use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

use anyhow::{Context, Result, anyhow, bail};
use arvalez_ir::{
    Attributes, CoreIr, Field, HttpMethod, Model, Operation, Parameter, ParameterLocation,
    RequestBody, Response, SourceRef, TypeRef, validate_ir,
};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Debug, Clone, Copy, Default)]
pub struct LoadOpenApiOptions {
    pub ignore_unhandled: bool,
}

#[derive(Debug, Clone)]
pub struct OpenApiLoadResult {
    pub ir: CoreIr,
    pub warnings: Vec<String>,
}

pub fn load_openapi_to_ir(path: impl AsRef<Path>) -> Result<CoreIr> {
    Ok(load_openapi_to_ir_with_options(path, LoadOpenApiOptions::default())?.ir)
}

pub fn load_openapi_to_ir_with_options(
    path: impl AsRef<Path>,
    options: LoadOpenApiOptions,
) -> Result<OpenApiLoadResult> {
    let path = path.as_ref();
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read OpenAPI document `{}`", path.display()))?;

    let document: OpenApiDocument = match path.extension().and_then(|ext| ext.to_str()) {
        Some("yaml") | Some("yml") => serde_yaml::from_str(&raw).with_context(|| {
            format!("failed to parse YAML OpenAPI document `{}`", path.display())
        })?,
        _ => serde_json::from_str(&raw).with_context(|| {
            format!("failed to parse JSON OpenAPI document `{}`", path.display())
        })?,
    };

    OpenApiImporter::new(document, options).build_ir()
}

struct OpenApiImporter {
    document: OpenApiDocument,
    models: BTreeMap<String, Model>,
    generated_model_names: BTreeSet<String>,
    warnings: Vec<String>,
    options: LoadOpenApiOptions,
}

impl OpenApiImporter {
    fn new(document: OpenApiDocument, options: LoadOpenApiOptions) -> Self {
        Self {
            document,
            models: BTreeMap::new(),
            generated_model_names: BTreeSet::new(),
            warnings: Vec::new(),
            options,
        }
    }

    fn build_ir(mut self) -> Result<OpenApiLoadResult> {
        self.import_component_models()?;

        let mut operations = Vec::new();
        let paths = self.document.paths.clone();
        for (path, item) in &paths {
            operations.extend(self.import_path_item(path, item)?);
        }

        let ir = CoreIr {
            models: self.models.into_values().collect(),
            operations,
            ..Default::default()
        };

        validate_ir(&ir).context("generated IR is invalid")?;
        Ok(OpenApiLoadResult {
            ir,
            warnings: self.warnings,
        })
    }

    fn import_component_models(&mut self) -> Result<()> {
        let schemas = self.document.components.schemas.clone();
        for (name, schema) in schemas {
            let pointer = format!("#/components/schemas/{name}");
            self.ensure_named_schema_model(&name, &schema, &pointer)?;
        }
        Ok(())
    }

    fn import_path_item(&mut self, path: &str, item: &PathItem) -> Result<Vec<Operation>> {
        let mut operations = Vec::new();
        let shared_parameters = item.parameters.clone().unwrap_or_default();
        let candidates = [
            (HttpMethod::Get, item.get.as_ref()),
            (HttpMethod::Post, item.post.as_ref()),
            (HttpMethod::Put, item.put.as_ref()),
            (HttpMethod::Patch, item.patch.as_ref()),
            (HttpMethod::Delete, item.delete.as_ref()),
        ];

        for (method, spec) in candidates {
            let Some(spec) = spec else {
                continue;
            };

            let operation_name = spec
                .operation_id
                .clone()
                .unwrap_or_else(|| fallback_operation_name(method, path));
            let mut operation = Operation {
                id: format!("operation.{operation_name}"),
                name: operation_name.clone(),
                method,
                path: path.to_owned(),
                params: Vec::new(),
                request_body: None,
                responses: Vec::new(),
                attributes: operation_attributes(spec),
                source: Some(SourceRef {
                    pointer: format!("#/paths/{}/{}", json_pointer_key(path), method_key(method)),
                    line: None,
                }),
            };

            for param in shared_parameters.iter().chain(spec.parameters.iter()) {
                operation.params.push(self.import_parameter(param)?);
            }

            if let Some(request_body) = &spec.request_body {
                operation.request_body =
                    Some(self.import_request_body(request_body, &operation_name, path, method)?);
            }

            for (status, response) in &spec.responses {
                operation.responses.push(self.import_response(
                    status,
                    response,
                    &operation_name,
                    path,
                    method,
                )?);
            }

            operations.push(operation);
        }

        Ok(operations)
    }

    fn import_parameter(&mut self, param: &ParameterSpec) -> Result<Parameter> {
        let imported = self.import_schema_type(
            &param.schema,
            &InlineModelContext::Parameter {
                name: param.name.clone(),
            },
        )?;

        Ok(Parameter {
            name: param.name.clone(),
            location: param.location,
            type_ref: imported
                .type_ref
                .unwrap_or_else(|| TypeRef::primitive("any")),
            required: param.required,
        })
    }

    fn import_request_body(
        &mut self,
        request_body: &RequestBodySpec,
        operation_name: &str,
        path: &str,
        method: HttpMethod,
    ) -> Result<RequestBody> {
        let (media_type, media_spec) =
            request_body.content.iter().next().ok_or_else(|| {
                anyhow!("request body for `{operation_name}` has no content entries")
            })?;

        let imported = media_spec
            .schema
            .as_ref()
            .map(|schema| {
                self.import_schema_type(
                    schema,
                    &InlineModelContext::RequestBody {
                        operation_name: operation_name.to_owned(),
                        pointer: format!(
                            "#/paths/{}/{}/requestBody/content/{}/schema",
                            json_pointer_key(path),
                            method_key(method),
                            json_pointer_key(media_type)
                        ),
                    },
                )
            })
            .transpose()?;

        Ok(RequestBody {
            required: request_body.required,
            media_type: media_type.clone(),
            type_ref: imported.and_then(|value| value.type_ref),
        })
    }

    fn import_response(
        &mut self,
        status: &str,
        response: &ResponseSpec,
        operation_name: &str,
        path: &str,
        method: HttpMethod,
    ) -> Result<Response> {
        let (media_type, schema) = response
            .content
            .iter()
            .find_map(|(media_type, media)| {
                media.schema.as_ref().map(|schema| (media_type, schema))
            })
            .map(|(media_type, schema)| (Some(media_type.clone()), Some(schema)))
            .unwrap_or((None, None));

        let imported = schema
            .map(|schema| {
                self.import_schema_type(
                    schema,
                    &InlineModelContext::Response {
                        operation_name: operation_name.to_owned(),
                        status: status.to_owned(),
                        pointer: media_type.as_ref().map_or_else(
                            || {
                                format!(
                                    "#/paths/{}/{}/responses/{}",
                                    json_pointer_key(path),
                                    method_key(method),
                                    json_pointer_key(status)
                                )
                            },
                            |media_type| {
                                format!(
                                    "#/paths/{}/{}/responses/{}/content/{}/schema",
                                    json_pointer_key(path),
                                    method_key(method),
                                    json_pointer_key(status),
                                    json_pointer_key(media_type)
                                )
                            }
                        ),
                    },
                )
            })
            .transpose()?;

        let mut attributes = Attributes::default();
        if !response.description.is_empty() {
            attributes.insert(
                "description".into(),
                Value::String(response.description.clone()),
            );
        }

        Ok(Response {
            status: status.to_owned(),
            media_type,
            type_ref: imported.and_then(|value| value.type_ref),
            attributes,
        })
    }

    fn ensure_named_schema_model(
        &mut self,
        name: &str,
        schema: &Schema,
        pointer: &str,
    ) -> Result<()> {
        if self.models.contains_key(name) {
            return Ok(());
        }

        let model = self.build_model_from_schema(name, schema, pointer)?;
        self.generated_model_names.insert(name.to_owned());
        self.models.insert(name.to_owned(), model);
        Ok(())
    }

    fn build_model_from_schema(
        &mut self,
        name: &str,
        schema: &Schema,
        pointer: &str,
    ) -> Result<Model> {
        self.validate_schema_keywords(schema, pointer)?;

        if let Some(enum_values) = &schema.enum_values {
            let mut model = Model::new(format!("model.{}", to_snake_case(name)), name.to_owned());
            model.source = Some(SourceRef {
                pointer: pointer.to_owned(),
                line: None,
            });
            model
                .attributes
                .insert("enum_values".into(), Value::Array(enum_values.clone()));
            if let Some(schema_type) = &schema.schema_type {
                model
                    .attributes
                    .insert("enum_base_type".into(), Value::String(schema_type.clone()));
            }
            if let Some(title) = &schema.title {
                model
                    .attributes
                    .insert("title".into(), Value::String(title.clone()));
            }
            return Ok(model);
        }

        if !schema_is_object_like(schema) {
            let imported = self.import_schema_type_inner(
                schema,
                &InlineModelContext::NamedSchema {
                    name: name.to_owned(),
                    pointer: pointer.to_owned(),
                },
                true,
            )?;
            let mut model = Model::new(format!("model.{}", to_snake_case(name)), name.to_owned());
            model.source = Some(SourceRef {
                pointer: pointer.to_owned(),
                line: None,
            });
            model.attributes.insert(
                "alias_type_ref".into(),
                json!(
                    imported
                        .type_ref
                        .unwrap_or_else(|| TypeRef::primitive("any"))
                ),
            );
            model
                .attributes
                .insert("alias_nullable".into(), Value::Bool(imported.nullable));
            if let Some(title) = &schema.title {
                model
                    .attributes
                    .insert("title".into(), Value::String(title.clone()));
            }
            return Ok(model);
        }

        let empty_properties = BTreeMap::new();
        let properties = schema.properties.as_ref().unwrap_or(&empty_properties);
        let required: BTreeSet<&str> = schema
            .required
            .iter()
            .flat_map(|items| items.iter().map(String::as_str))
            .collect();

        let mut model = Model::new(format!("model.{}", to_snake_case(name)), name.to_owned());
        model.source = Some(SourceRef {
            pointer: pointer.to_owned(),
            line: None,
        });
        if let Some(title) = &schema.title {
            model
                .attributes
                .insert("title".into(), Value::String(title.clone()));
        }

        for (field_name, property_schema) in properties {
            let imported = self.import_schema_type(
                property_schema,
                &InlineModelContext::Field {
                    model_name: name.to_owned(),
                    field_name: field_name.clone(),
                    pointer: format!(
                        "{}/properties/{}",
                        pointer,
                        json_pointer_key(field_name)
                    ),
                },
            )?;
            let mut field = Field::new(
                field_name.clone(),
                imported
                    .type_ref
                    .unwrap_or_else(|| TypeRef::primitive("any")),
            );
            field.optional = !required.contains(field_name.as_str());
            field.nullable = imported.nullable;
            model.fields.push(field);
        }

        Ok(model)
    }

    fn import_schema_type(
        &mut self,
        schema: &Schema,
        context: &InlineModelContext,
    ) -> Result<ImportedType> {
        self.import_schema_type_inner(schema, context, false)
    }

    fn import_schema_type_inner(
        &mut self,
        schema: &Schema,
        context: &InlineModelContext,
        skip_keyword_validation: bool,
    ) -> Result<ImportedType> {
        if !skip_keyword_validation {
            self.validate_schema_keywords(schema, &context.describe())?;
        }

        if let Some(reference) = &schema.reference {
            return Ok(ImportedType {
                type_ref: Some(TypeRef::named(ref_name(reference)?)),
                nullable: false,
            });
        }

        if let Some(const_value) = &schema.const_value {
            return self.import_const_type(schema, const_value, context);
        }

        if let Some(any_of) = &schema.any_of {
            return self.import_any_of(any_of, context);
        }

        if let Some(one_of) = &schema.one_of {
            return self.import_any_of(one_of, context);
        }

        if let Some(schema_type) = &schema.schema_type {
            return match schema_type.as_str() {
                "string" => {
                    if schema.format.as_deref() == Some("binary") {
                        Ok(ImportedType::plain(TypeRef::primitive("binary")))
                    } else {
                        Ok(ImportedType::plain(TypeRef::primitive("string")))
                    }
                }
                "integer" => Ok(ImportedType::plain(TypeRef::primitive("integer"))),
                "number" => Ok(ImportedType::plain(TypeRef::primitive("number"))),
                "boolean" => Ok(ImportedType::plain(TypeRef::primitive("boolean"))),
                "array" => {
                    let item_schema = schema.items.as_ref().ok_or_else(|| {
                        anyhow!("{}: array schema is missing `items`", context.describe())
                    })?;
                    let imported = self.import_schema_type(item_schema, context)?;
                    Ok(ImportedType::plain(TypeRef::array(
                        imported
                            .type_ref
                            .unwrap_or_else(|| TypeRef::primitive("any")),
                    )))
                }
                "object" => self.import_object_type(schema, context),
                other => {
                    self.handle_unhandled(
                        &context.describe(),
                        format!("unsupported schema type `{other}`"),
                    )?;
                    Ok(ImportedType::plain(TypeRef::primitive("any")))
                }
            };
        }

        if is_unconstrained_schema(schema) {
            return Ok(ImportedType::plain(TypeRef::primitive("any")));
        }

        if schema.properties.is_some() || schema.additional_properties.is_some() {
            return self.import_object_type(schema, context);
        }

        self.handle_unhandled(&context.describe(), "schema shape is not supported yet")?;
        Ok(ImportedType::plain(TypeRef::primitive("any")))
    }

    fn validate_schema_keywords(&mut self, schema: &Schema, context: &str) -> Result<()> {
        if schema.all_of.is_some() {
            self.handle_unhandled(context, "`allOf` is not supported yet")?;
        }

        for keyword in schema.extra_keywords.keys() {
            if is_known_ignored_schema_keyword(keyword) || keyword.starts_with("x-") {
                continue;
            }

            if is_known_but_unimplemented_schema_keyword(keyword) {
                self.handle_unhandled(context, format!("`{keyword}` is not supported yet"))?;
                continue;
            }

            self.handle_unhandled(context, format!("unknown schema keyword `{keyword}`"))?;
        }

        Ok(())
    }

    fn import_const_type(
        &mut self,
        schema: &Schema,
        const_value: &Value,
        context: &InlineModelContext,
    ) -> Result<ImportedType> {
        if let Some(schema_type) = &schema.schema_type {
            let imported = match schema_type.as_str() {
                "string" => {
                    if schema.format.as_deref() == Some("binary") {
                        ImportedType::plain(TypeRef::primitive("binary"))
                    } else {
                        ImportedType::plain(TypeRef::primitive("string"))
                    }
                }
                "integer" => ImportedType::plain(TypeRef::primitive("integer")),
                "number" => ImportedType::plain(TypeRef::primitive("number")),
                "boolean" => ImportedType::plain(TypeRef::primitive("boolean")),
                "null" => ImportedType {
                    type_ref: Some(TypeRef::primitive("any")),
                    nullable: true,
                },
                "array" => {
                    let item_schema = schema.items.as_ref().ok_or_else(|| {
                        anyhow!("{}: array schema is missing `items`", context.describe())
                    })?;
                    let imported = self.import_schema_type(item_schema, context)?;
                    ImportedType::plain(TypeRef::array(
                        imported
                            .type_ref
                            .unwrap_or_else(|| TypeRef::primitive("any")),
                    ))
                }
                "object" => self.import_object_type(schema, context)?,
                other => {
                    self.handle_unhandled(
                        &context.describe(),
                        format!("unsupported schema type `{other}`"),
                    )?;
                    ImportedType::plain(TypeRef::primitive("any"))
                }
            };
            return Ok(imported);
        }

        let imported = match const_value {
            Value::String(_) => ImportedType::plain(TypeRef::primitive("string")),
            Value::Bool(_) => ImportedType::plain(TypeRef::primitive("boolean")),
            Value::Number(number) => {
                if number.is_i64() || number.is_u64() {
                    ImportedType::plain(TypeRef::primitive("integer"))
                } else {
                    ImportedType::plain(TypeRef::primitive("number"))
                }
            }
            Value::Null => ImportedType {
                type_ref: Some(TypeRef::primitive("any")),
                nullable: true,
            },
            Value::Array(_) => {
                if let Some(items) = &schema.items {
                    let imported = self.import_schema_type(items, context)?;
                    ImportedType::plain(TypeRef::array(
                        imported
                            .type_ref
                            .unwrap_or_else(|| TypeRef::primitive("any")),
                    ))
                } else {
                    ImportedType::plain(TypeRef::array(TypeRef::primitive("any")))
                }
            }
            Value::Object(_) => self.import_object_type(schema, context)?,
        };

        Ok(imported)
    }

    fn import_any_of(
        &mut self,
        schemas: &[Schema],
        context: &InlineModelContext,
    ) -> Result<ImportedType> {
        let mut variants = Vec::new();
        let mut nullable = false;

        for schema in schemas {
            if schema.schema_type.as_deref() == Some("null") {
                nullable = true;
                continue;
            }

            let imported = self.import_schema_type(schema, context)?;
            if imported.nullable {
                nullable = true;
            }
            if let Some(type_ref) = imported.type_ref {
                variants.push(type_ref);
            }
        }

        variants = dedupe_variants(variants);
        let type_ref = match variants.len() {
            0 => Some(TypeRef::primitive("any")),
            1 => variants.into_iter().next(),
            _ => Some(TypeRef::Union { variants }),
        };

        Ok(ImportedType { type_ref, nullable })
    }

    fn import_object_type(
        &mut self,
        schema: &Schema,
        context: &InlineModelContext,
    ) -> Result<ImportedType> {
        if let Some(additional_properties) = &schema.additional_properties {
            match additional_properties {
                AdditionalProperties::Schema(additional_properties) => {
                    let imported = self.import_schema_type(additional_properties, context)?;
                    return Ok(ImportedType::plain(TypeRef::map(
                        imported
                            .type_ref
                            .unwrap_or_else(|| TypeRef::primitive("any")),
                    )));
                }
                AdditionalProperties::Bool(true) => {
                    return Ok(ImportedType::plain(TypeRef::map(TypeRef::primitive("any"))));
                }
                AdditionalProperties::Bool(false) => {}
            }
        }

        if schema.properties.is_some() {
            let model_name = self.inline_model_name(schema, context);
            if !self.models.contains_key(&model_name) {
                let pointer = context.synthetic_pointer(&model_name);
                let model = self.build_model_from_schema(&model_name, schema, &pointer)?;
                self.generated_model_names.insert(model_name.clone());
                self.models.insert(model_name.clone(), model);
            }
            return Ok(ImportedType::plain(TypeRef::named(model_name)));
        }

        Ok(ImportedType::plain(TypeRef::primitive("object")))
    }

    fn inline_model_name(&mut self, schema: &Schema, context: &InlineModelContext) -> String {
        let base = schema.title.clone().unwrap_or_else(|| context.name_hint());
        let candidate = to_pascal_case(&base);
        if self.generated_model_names.insert(candidate.clone()) {
            return candidate;
        }

        let mut index = 2usize;
        loop {
            let candidate = format!("{candidate}{index}");
            if self.generated_model_names.insert(candidate.clone()) {
                return candidate;
            }
            index += 1;
        }
    }

    fn handle_unhandled(&mut self, context: &str, message: impl Into<String>) -> Result<()> {
        let message = format!("{context}: {}", message.into());
        if self.options.ignore_unhandled {
            self.warnings.push(message);
            Ok(())
        } else {
            bail!(message)
        }
    }
}

#[derive(Debug, Clone)]
struct ImportedType {
    type_ref: Option<TypeRef>,
    nullable: bool,
}

impl ImportedType {
    fn plain(type_ref: TypeRef) -> Self {
        Self {
            type_ref: Some(type_ref),
            nullable: false,
        }
    }
}

#[derive(Debug)]
enum InlineModelContext {
    NamedSchema {
        name: String,
        pointer: String,
    },
    Field {
        model_name: String,
        field_name: String,
        pointer: String,
    },
    RequestBody {
        operation_name: String,
        pointer: String,
    },
    Response {
        operation_name: String,
        status: String,
        pointer: String,
    },
    Parameter {
        name: String,
    },
}

impl InlineModelContext {
    fn name_hint(&self) -> String {
        match self {
            Self::NamedSchema { name, .. } => name.clone(),
            Self::Field {
                model_name,
                field_name,
                ..
            } => format!("{model_name} {field_name}"),
            Self::RequestBody { operation_name, .. } => format!("{operation_name} request"),
            Self::Response {
                operation_name,
                status,
                ..
            } => format!("{operation_name} {status} response"),
            Self::Parameter { name } => format!("{name} param"),
        }
    }

    fn describe(&self) -> String {
        match self {
            InlineModelContext::NamedSchema { pointer, .. } => pointer.clone(),
            InlineModelContext::Field {
                pointer,
                ..
            } => pointer.clone(),
            InlineModelContext::RequestBody { pointer, .. } => pointer.clone(),
            InlineModelContext::Response {
                pointer, ..
            } => pointer.clone(),
            InlineModelContext::Parameter { name } => format!("parameter `{name}`"),
        }
    }

    fn synthetic_pointer(&self, model_name: &str) -> String {
        match self {
            Self::NamedSchema { pointer, .. } => pointer.clone(),
            Self::Field {
                pointer, ..
            } => pointer.clone(),
            Self::RequestBody { pointer, .. } => pointer.clone(),
            Self::Response {
                pointer, ..
            } => pointer.clone(),
            Self::Parameter { name } => format!("#/synthetic/parameters/{name}/{model_name}"),
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
struct OpenApiDocument {
    #[serde(default)]
    paths: BTreeMap<String, PathItem>,
    #[serde(default)]
    components: Components,
}

#[derive(Debug, Deserialize, Default, Clone)]
struct Components {
    #[serde(default)]
    schemas: BTreeMap<String, Schema>,
}

#[derive(Debug, Deserialize, Default, Clone)]
struct PathItem {
    #[serde(default)]
    parameters: Option<Vec<ParameterSpec>>,
    #[serde(default)]
    get: Option<OperationSpec>,
    #[serde(default)]
    post: Option<OperationSpec>,
    #[serde(default)]
    put: Option<OperationSpec>,
    #[serde(default)]
    patch: Option<OperationSpec>,
    #[serde(default)]
    delete: Option<OperationSpec>,
}

#[derive(Debug, Deserialize, Default, Clone)]
struct OperationSpec {
    #[serde(rename = "operationId")]
    #[serde(default)]
    operation_id: Option<String>,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    parameters: Vec<ParameterSpec>,
    #[serde(rename = "requestBody")]
    #[serde(default)]
    request_body: Option<RequestBodySpec>,
    #[serde(default)]
    responses: BTreeMap<String, ResponseSpec>,
}

#[derive(Debug, Deserialize, Clone)]
struct ParameterSpec {
    name: String,
    #[serde(rename = "in")]
    location: ParameterLocation,
    #[serde(default)]
    required: bool,
    schema: Schema,
}

#[derive(Debug, Deserialize, Default, Clone)]
struct RequestBodySpec {
    #[serde(default)]
    required: bool,
    #[serde(default)]
    content: BTreeMap<String, MediaTypeSpec>,
}

#[derive(Debug, Deserialize, Default, Clone)]
struct ResponseSpec {
    #[serde(default)]
    description: String,
    #[serde(default)]
    content: BTreeMap<String, MediaTypeSpec>,
}

#[derive(Debug, Deserialize, Default, Clone)]
struct MediaTypeSpec {
    #[serde(default)]
    schema: Option<Schema>,
}

#[derive(Debug, Deserialize, Default, Clone)]
struct Schema {
    #[serde(rename = "$ref")]
    #[serde(default)]
    reference: Option<String>,
    #[serde(rename = "type")]
    #[serde(default)]
    schema_type: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    format: Option<String>,
    #[serde(rename = "const")]
    #[serde(default)]
    const_value: Option<Value>,
    #[serde(rename = "discriminator")]
    #[serde(default)]
    _discriminator: Option<Value>,
    #[serde(rename = "allOf")]
    #[serde(default)]
    all_of: Option<Vec<Schema>>,
    #[serde(rename = "enum")]
    #[serde(default)]
    enum_values: Option<Vec<Value>>,
    #[serde(default)]
    properties: Option<BTreeMap<String, Schema>>,
    #[serde(default)]
    required: Option<Vec<String>>,
    #[serde(default)]
    items: Option<Box<Schema>>,
    #[serde(rename = "additionalProperties")]
    #[serde(default)]
    additional_properties: Option<AdditionalProperties>,
    #[serde(rename = "anyOf")]
    #[serde(default)]
    any_of: Option<Vec<Schema>>,
    #[serde(rename = "oneOf")]
    #[serde(default)]
    one_of: Option<Vec<Schema>>,
    #[serde(flatten)]
    #[serde(default)]
    extra_keywords: BTreeMap<String, Value>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(untagged)]
enum AdditionalProperties {
    Bool(bool),
    Schema(Box<Schema>),
}

fn ref_name(reference: &str) -> Result<String> {
    reference
        .rsplit('/')
        .next()
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| anyhow!("unsupported reference `{reference}`"))
}

fn schema_is_object_like(schema: &Schema) -> bool {
    matches!(schema.schema_type.as_deref(), Some("object"))
        || schema.properties.is_some()
        || schema.additional_properties.is_some()
}

fn is_unconstrained_schema(schema: &Schema) -> bool {
    schema.reference.is_none()
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
        && schema.extra_keywords.is_empty()
}

fn is_known_ignored_schema_keyword(keyword: &str) -> bool {
    matches!(
        keyword,
        "default"
            | "description"
            | "example"
            | "examples"
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

fn is_known_but_unimplemented_schema_keyword(keyword: &str) -> bool {
    matches!(
        keyword,
        "not"
            | "if"
            | "then"
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

fn fallback_operation_name(method: HttpMethod, path: &str) -> String {
    to_snake_case(&format!("{} {}", method_key(method), path))
}

fn method_key(method: HttpMethod) -> &'static str {
    match method {
        HttpMethod::Get => "get",
        HttpMethod::Post => "post",
        HttpMethod::Put => "put",
        HttpMethod::Patch => "patch",
        HttpMethod::Delete => "delete",
    }
}

fn operation_attributes(spec: &OperationSpec) -> Attributes {
    let mut attributes = Attributes::default();
    if let Some(summary) = &spec.summary {
        attributes.insert("summary".into(), Value::String(summary.clone()));
    }
    if !spec.tags.is_empty() {
        attributes.insert("tags".into(), json!(spec.tags));
    }
    attributes
}

fn json_pointer_key(input: &str) -> String {
    input.replace('~', "~0").replace('/', "~1")
}

fn to_pascal_case(input: &str) -> String {
    let mut output = String::new();
    for part in split_words(input) {
        let mut chars = part.chars();
        if let Some(first) = chars.next() {
            output.extend(first.to_uppercase());
            output.push_str(chars.as_str());
        }
    }
    if output.is_empty() {
        "InlineModel".into()
    } else {
        output
    }
}

fn to_snake_case(input: &str) -> String {
    let parts = split_words(input);
    if parts.is_empty() {
        return "value".into();
    }
    parts.join("_").to_lowercase()
}

fn split_words(input: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();

    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() {
            if ch.is_uppercase() && !current.is_empty() {
                words.push(current.clone());
                current.clear();
            }
            current.push(ch.to_ascii_lowercase());
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

fn dedupe_variants(variants: Vec<TypeRef>) -> Vec<TypeRef> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imports_minimal_openapi_document() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {
    "/widgets/{widget_id}": {
      "get": {
        "operationId": "get_widget",
        "parameters": [
          {
            "name": "widget_id",
            "in": "path",
            "required": true,
            "schema": { "type": "string" }
          }
        ],
        "responses": {
          "200": {
            "description": "ok",
            "content": {
              "application/json": {
                "schema": { "$ref": "#/components/schemas/Widget" }
              }
            }
          }
        }
      }
    }
  },
  "components": {
    "schemas": {
      "Widget": {
        "type": "object",
        "required": ["id"],
        "properties": {
          "status": {
            "$ref": "#/components/schemas/WidgetStatus"
          },
          "id": { "type": "string" },
          "count": { "anyOf": [{ "type": "integer" }, { "type": "null" }] },
          "labels": {
            "type": "object",
            "additionalProperties": { "type": "string" }
          },
          "metadata": {
            "type": "object",
            "additionalProperties": true
          }
        }
      },
      "WidgetStatus": {
        "type": "string",
        "enum": ["READY", "PAUSED"]
      }
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid test spec");
        let result = OpenApiImporter::new(document, LoadOpenApiOptions::default())
            .build_ir()
            .expect("should import successfully");
        let ir = result.ir;

        assert_eq!(ir.models.len(), 2);
        assert_eq!(ir.operations.len(), 1);
        assert_eq!(ir.operations[0].name, "get_widget");
        assert!(ir.models.iter().any(|model| model.name == "Widget"));
        assert!(ir.models.iter().any(|model| model.name == "WidgetStatus"));
        let widget = ir
            .models
            .iter()
            .find(|model| model.name == "Widget")
            .expect("widget model");
        assert!(
            widget
                .fields
                .iter()
                .find(|field| field.name == "count")
                .expect("count field")
                .nullable
        );
        assert!(matches!(
            widget
                .fields
                .iter()
                .find(|field| field.name == "metadata")
                .expect("metadata field")
                .type_ref,
            TypeRef::Map { .. }
        ));
        assert_eq!(
            widget
                .fields
                .iter()
                .find(|field| field.name == "status")
                .expect("status field")
                .type_ref,
            TypeRef::named("WidgetStatus")
        );
    }

    #[test]
    fn supports_const_scalar_fields() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {},
  "components": {
    "schemas": {
      "PatchOp": {
        "type": "object",
        "properties": {
          "op": {
            "type": "string",
            "const": "replace"
          }
        }
      }
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid test spec");
        let result = OpenApiImporter::new(document, LoadOpenApiOptions::default())
            .build_ir()
            .expect("const should be supported");
        let patch_op = result
            .ir
            .models
            .iter()
            .find(|model| model.name == "PatchOp")
            .expect("PatchOp model");
        assert!(
            patch_op
                .fields
                .iter()
                .any(|field| field.name == "op" && field.type_ref == TypeRef::primitive("string"))
        );
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn supports_discriminator_on_unions() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {},
  "components": {
    "schemas": {
      "AddOperation": {
        "type": "object",
        "properties": {
          "op": {
            "type": "string",
            "const": "add"
          }
        }
      },
      "RemoveOperation": {
        "type": "object",
        "properties": {
          "op": {
            "type": "string",
            "const": "remove"
          }
        }
      },
      "PatchSchema": {
        "type": "object",
        "properties": {
          "patches": {
            "type": "array",
            "items": {
              "oneOf": [
                { "$ref": "#/components/schemas/AddOperation" },
                { "$ref": "#/components/schemas/RemoveOperation" }
              ],
              "discriminator": {
                "propertyName": "op",
                "mapping": {
                  "add": "#/components/schemas/AddOperation",
                  "remove": "#/components/schemas/RemoveOperation"
                }
              }
            }
          }
        }
      }
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid test spec");
        let result = OpenApiImporter::new(document, LoadOpenApiOptions::default())
            .build_ir()
            .expect("discriminator unions should be supported");
        let patch_schema = result
            .ir
            .models
            .iter()
            .find(|model| model.name == "PatchSchema")
            .expect("PatchSchema model");
        let patches = patch_schema
            .fields
            .iter()
            .find(|field| field.name == "patches")
            .expect("patches field");
        assert!(
            matches!(
                &patches.type_ref,
                TypeRef::Array { item }
                    if matches!(
                        item.as_ref(),
                        TypeRef::Union { variants }
                            if variants == &vec![
                                TypeRef::named("AddOperation"),
                                TypeRef::named("RemoveOperation")
                            ]
                    )
            )
        );
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn errors_on_unhandled_elements_by_default_and_warns_when_ignored() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {},
  "components": {
    "schemas": {
      "PatchSchema": {
        "allOf": [
          { "type": "object" }
        ]
      }
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid test spec");
        let strict_error = OpenApiImporter::new(document.clone(), LoadOpenApiOptions::default())
            .build_ir()
            .expect_err("strict mode should fail");
        assert!(
            strict_error
                .to_string()
                .contains("`allOf` is not supported yet")
        );

        let warning_result = OpenApiImporter::new(
            document,
            LoadOpenApiOptions {
                ignore_unhandled: true,
            },
        )
        .build_ir()
        .expect("ignore mode should succeed");
        assert!(
            warning_result
                .warnings
                .iter()
                .any(|warning| warning.contains("`allOf` is not supported yet"))
        );
    }

    #[test]
    fn errors_on_unknown_schema_keywords() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {},
  "components": {
    "schemas": {
      "PatchSchema": {
        "type": "string",
        "frobnicate": true
      }
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid test spec");
        let error = OpenApiImporter::new(document, LoadOpenApiOptions::default())
            .build_ir()
            .expect_err("unknown keyword should fail");
        assert!(
            error
                .to_string()
                .contains("unknown schema keyword `frobnicate`")
        );
    }

    #[test]
    fn ignores_known_non_codegen_schema_keywords() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {},
  "components": {
    "schemas": {
      "PatchSchema": {
        "type": "string",
        "description": "some text",
        "default": "value",
        "minLength": 1
      }
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid test spec");
        let result = OpenApiImporter::new(document, LoadOpenApiOptions::default())
            .build_ir()
            .expect("known ignored keywords should not fail");
        assert!(result.warnings.is_empty());
    }
}
