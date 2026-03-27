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
use indexmap::IndexMap;
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

    let loaded = match path.extension().and_then(|ext| ext.to_str()) {
        Some("yaml") | Some("yml") => parse_yaml_openapi_document(path, &raw)?,
        _ => parse_json_openapi_document(path, &raw)?,
    };

    OpenApiImporter::new(loaded.document, loaded.source, options).build_ir()
}

fn parse_json_openapi_document(path: &Path, raw: &str) -> Result<LoadedOpenApiDocument> {
    let source_value: Value = serde_json::from_str(raw).with_context(|| {
        format!("failed to parse JSON OpenAPI document `{}`", path.display())
    })?;
    let mut deserializer = serde_json::Deserializer::from_str(raw);
    let document = serde_path_to_error::deserialize(&mut deserializer).map_err(|error| {
        let schema_path = error.path().to_string();
        let inner = error.into_inner();
        let line = inner.line();
        let column = inner.column();
        let message = inner.to_string();
        anyhow!(format_openapi_deserialize_error(
            "JSON",
            path,
            raw,
            if schema_path.is_empty() {
                None
            } else {
                Some(schema_path.as_str())
            },
            line,
            column,
            &message,
        ))
    })?;

    Ok(LoadedOpenApiDocument {
        document,
        source: OpenApiSource {
            format: SourceFormat::Json,
            value: source_value,
        },
    })
}

fn parse_yaml_openapi_document(path: &Path, raw: &str) -> Result<LoadedOpenApiDocument> {
    let yaml_value: serde_yaml::Value = serde_yaml::from_str(raw)
        .with_context(|| format!("failed to parse YAML OpenAPI document `{}`", path.display()))?;
    let source_value: Value = serde_json::to_value(yaml_value).with_context(|| {
        format!(
            "failed to convert YAML OpenAPI document `{}` into preview data",
            path.display()
        )
    })?;

    let document = serde_yaml::from_str(raw).map_err(|error| {
        let (line, column) = error
            .location()
            .map(|location| (location.line(), location.column()))
            .unwrap_or((0, 0));
        anyhow!(format_openapi_deserialize_error(
            "YAML",
            path,
            raw,
            None,
            line,
            column,
            &error.to_string(),
        ))
    })?;

    Ok(LoadedOpenApiDocument {
        document,
        source: OpenApiSource {
            format: SourceFormat::Yaml,
            value: source_value,
        },
    })
}

fn format_openapi_deserialize_error(
    format_name: &str,
    path: &Path,
    raw: &str,
    schema_path: Option<&str>,
    line: usize,
    column: usize,
    message: &str,
) -> String {
    let mut rendered = format!(
        "failed to parse {format_name} OpenAPI document `{}`",
        path.display()
    );
    rendered.push_str("\nCaused by:");

    if let Some(schema_path) = schema_path {
        rendered.push_str(&format!(
            "\n  schema mismatch at `{schema_path}`: {message}"
        ));
    } else {
        rendered.push_str(&format!("\n  {message}"));
    }

    if line > 0 && column > 0 {
        rendered.push_str(&format!("\n  location: line {line}, column {column}"));
        if let Some(source_line) = raw.lines().nth(line.saturating_sub(1)) {
            rendered.push_str(&format!("\n  source: {source_line}"));
            rendered.push_str(&format!(
                "\n          {}^",
                " ".repeat(column.saturating_sub(1))
            ));
        }
    }

    rendered.push_str(
        "\n  note: this usually means the document is valid JSON/YAML, but an OpenAPI field had an unexpected shape.",
    );
    rendered
}

struct OpenApiImporter {
    document: OpenApiDocument,
    source: OpenApiSource,
    models: BTreeMap<String, Model>,
    generated_model_names: BTreeSet<String>,
    normalized_all_of_refs: BTreeMap<String, Schema>,
    active_all_of_refs: Vec<String>,
    warnings: Vec<String>,
    options: LoadOpenApiOptions,
}

impl OpenApiImporter {
    fn new(document: OpenApiDocument, source: OpenApiSource, options: LoadOpenApiOptions) -> Self {
        Self {
            document,
            source,
            models: BTreeMap::new(),
            generated_model_names: BTreeSet::new(),
            normalized_all_of_refs: BTreeMap::new(),
            active_all_of_refs: Vec::new(),
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

    fn import_parameter(&mut self, param: &ParameterOrRef) -> Result<Parameter> {
        let param = self.resolve_parameter(param)?;
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

    fn resolve_parameter(&self, param: &ParameterOrRef) -> Result<ParameterSpec> {
        match param {
            ParameterOrRef::Inline(param) => Ok(param.clone()),
            ParameterOrRef::Ref { reference } => {
                let name = ref_name(reference)?;
                self.document
                    .components
                    .parameters
                    .get(&name)
                    .cloned()
                    .ok_or_else(|| anyhow!("unsupported reference `{reference}`"))
            }
        }
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
                anyhow!(self.format_pointer_error(
                    &format!(
                        "#/paths/{}/{}/requestBody/content",
                        json_pointer_key(path),
                        method_key(method)
                    ),
                    "request body has no content entries",
                    "Arvalez expects at least one media type under `requestBody.content`.",
                ))
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
        let schema = self.normalize_schema(schema, pointer)?;
        self.validate_schema_keywords(&schema, pointer)?;

        if let Some(enum_values) = &schema.enum_values {
            let mut model = Model::new(format!("model.{}", to_snake_case(name)), name.to_owned());
            model.source = Some(SourceRef {
                pointer: pointer.to_owned(),
                line: None,
            });
            model
                .attributes
                .insert("enum_values".into(), Value::Array(enum_values.clone()));
            if let Some(schema_type) = schema.primary_schema_type() {
                model
                    .attributes
                    .insert("enum_base_type".into(), Value::String(schema_type.to_owned()));
            }
            if let Some(title) = &schema.title {
                model
                    .attributes
                    .insert("title".into(), Value::String(title.clone()));
            }
            return Ok(model);
        }

        if !schema_is_object_like(&schema) {
            let imported = self.import_schema_type_inner(
                &schema,
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

        let empty_properties = IndexMap::new();
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
        let schema = self.normalize_schema(schema, &context.describe())?;

        if !skip_keyword_validation {
            self.validate_schema_keywords(&schema, &context.describe())?;
        }

        if let Some(reference) = &schema.reference {
            return Ok(ImportedType {
                type_ref: Some(TypeRef::named(ref_name(reference)?)),
                nullable: false,
            });
        }

        if let Some(const_value) = &schema.const_value {
            return self.import_const_type(&schema, const_value, context);
        }

        if let Some(any_of) = &schema.any_of {
            return self.import_any_of(any_of, context);
        }

        if let Some(one_of) = &schema.one_of {
            return self.import_any_of(one_of, context);
        }

        if let Some(imported) = self.import_schema_type_from_decl(&schema, context)? {
            return Ok(imported);
        }

        if is_unconstrained_schema(&schema) {
            return Ok(ImportedType::plain(TypeRef::primitive("any")));
        }

        if schema.properties.is_some() || schema.additional_properties.is_some() {
            return self.import_object_type(&schema, context);
        }

        self.handle_unhandled(&context.describe(), "schema shape is not supported yet")?;
        Ok(ImportedType::plain(TypeRef::primitive("any")))
    }

    fn import_schema_type_from_decl(
        &mut self,
        schema: &Schema,
        context: &InlineModelContext,
    ) -> Result<Option<ImportedType>> {
        let Some(schema_types) = &schema.schema_type else {
            return Ok(None);
        };

        let variants = schema_types.as_slice();
        if variants.len() == 1 {
            let schema_type = variants[0].as_str();
            return Ok(Some(match schema_type {
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
                "array" => {
                    let item_schema = schema.items.as_ref().ok_or_else(|| {
                        anyhow!(self.format_context_error(
                            &context.describe(),
                            "array schema is missing `items`",
                            "Add an `items` schema to describe the array element type.",
                        ))
                    })?;
                    let imported = self.import_schema_type(item_schema, context)?;
                    ImportedType::plain(TypeRef::array(
                        imported
                            .type_ref
                            .unwrap_or_else(|| TypeRef::primitive("any")),
                    ))
                }
                "object" => self.import_object_type(schema, context)?,
                "null" => ImportedType {
                    type_ref: Some(TypeRef::primitive("any")),
                    nullable: true,
                },
                other => {
                    self.handle_unhandled(
                        &context.describe(),
                        format!("unsupported schema type `{other}`"),
                    )?;
                    ImportedType::plain(TypeRef::primitive("any"))
                }
            }));
        }

        let mut nullable = false;
        let mut type_refs = Vec::new();
        for schema_type in variants {
            match schema_type.as_str() {
                "null" => nullable = true,
                other => {
                    let mut synthetic = schema.clone();
                    synthetic.schema_type = Some(SchemaTypeDecl::Single(other.to_owned()));
                    let imported = self
                        .import_schema_type_from_decl(&synthetic, context)?
                        .expect("single schema type should import");
                    if imported.nullable {
                        nullable = true;
                    }
                    if let Some(type_ref) = imported.type_ref {
                        type_refs.push(type_ref);
                    }
                }
            }
        }

        let type_refs = dedupe_variants(type_refs);
        let type_ref = match type_refs.len() {
            0 => Some(TypeRef::primitive("any")),
            1 => type_refs.into_iter().next(),
            _ => Some(TypeRef::Union { variants: type_refs }),
        };

        Ok(Some(ImportedType { type_ref, nullable }))
    }

    fn validate_schema_keywords(&mut self, schema: &Schema, context: &str) -> Result<()> {
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

    fn normalize_schema(&mut self, schema: &Schema, context: &str) -> Result<Schema> {
        if schema.all_of.is_some() {
            self.expand_all_of_schema(schema, context)
        } else {
            Ok(schema.clone())
        }
    }

    fn expand_all_of_schema(&mut self, schema: &Schema, context: &str) -> Result<Schema> {
        let mut merged = Schema {
            all_of: None,
            ..schema.clone()
        };

        for member in schema.all_of.clone().unwrap_or_default() {
            let resolved_member = self.resolve_schema_for_merge(&member, context)?;
            merged = self.merge_schemas(merged, resolved_member, context)?;
        }

        Ok(merged)
    }

    fn resolve_schema_for_merge(&mut self, schema: &Schema, context: &str) -> Result<Schema> {
        let mut resolved = if let Some(reference) = &schema.reference {
            self.resolve_schema_reference_for_all_of(reference, context)?
        } else {
            schema.clone()
        };

        if resolved.all_of.is_some() {
            resolved = self.expand_all_of_schema(&resolved, context)?;
        }

        if schema.reference.is_some() {
            let mut overlay = schema.clone();
            overlay.reference = None;
            overlay.all_of = None;
            resolved = self.merge_schemas(resolved, overlay, context)?;
        }

        Ok(resolved)
    }

    fn resolve_schema_reference_for_all_of(
        &mut self,
        reference: &str,
        context: &str,
    ) -> Result<Schema> {
        if let Some(cached) = self.normalized_all_of_refs.get(reference) {
            return Ok(cached.clone());
        }

        if self.active_all_of_refs.iter().any(|item| item == reference) {
            self.handle_unhandled(
                context,
                format!("`allOf` contains a recursive reference cycle involving `{reference}`"),
            )?;
            return Ok(Schema::default());
        }

        self.active_all_of_refs.push(reference.to_owned());
        let result: Result<Schema> = (|| {
            let mut resolved = self.resolve_schema_reference(reference)?;
            if resolved.all_of.is_some() {
                resolved = self.expand_all_of_schema(&resolved, reference)?;
            }
            Ok(resolved)
        })();
        self.active_all_of_refs.pop();

        let resolved = result?;
        self.normalized_all_of_refs
            .insert(reference.to_owned(), resolved.clone());
        Ok(resolved)
    }

    fn resolve_schema_reference(&self, reference: &str) -> Result<Schema> {
        let name = ref_name(reference)?;
        self.document
            .components
            .schemas
            .get(&name)
            .cloned()
            .ok_or_else(|| anyhow!("unsupported reference `{reference}`"))
    }

    fn merge_schemas(
        &mut self,
        mut base: Schema,
        overlay: Schema,
        context: &str,
    ) -> Result<Schema> {
        merge_optional_field(&mut base.title, overlay.title, "title", context, self)?;
        merge_optional_field(&mut base.format, overlay.format, "format", context, self)?;
        merge_optional_field(
            &mut base.schema_type,
            overlay.schema_type,
            "type",
            context,
            self,
        )?;
        merge_optional_field(&mut base.const_value, overlay.const_value, "const", context, self)?;
        merge_optional_field(
            &mut base._discriminator,
            overlay._discriminator,
            "discriminator",
            context,
            self,
        )?;
        merge_optional_field(
            &mut base.enum_values,
            overlay.enum_values,
            "enum",
            context,
            self,
        )?;
        merge_optional_field(&mut base.any_of, overlay.any_of, "anyOf", context, self)?;
        merge_optional_field(&mut base.one_of, overlay.one_of, "oneOf", context, self)?;

        let base_required = base.required.take();
        let overlay_required = overlay.required;
        base.required = match (base_required, overlay_required) {
            (None, None) => None,
            (left, right) => Some(merge_required(
                left.unwrap_or_default(),
                right.unwrap_or_default(),
            )),
        };

        match (base.items.take(), overlay.items) {
            (Some(left), Some(right)) => {
                base.items = Some(Box::new(self.merge_schemas(*left, *right, context)?));
            }
            (Some(left), None) => base.items = Some(left),
            (None, Some(right)) => base.items = Some(right),
            (None, None) => {}
        }

        match (base.additional_properties.take(), overlay.additional_properties) {
            (
                Some(AdditionalProperties::Schema(left)),
                Some(AdditionalProperties::Schema(right)),
            ) => {
                base.additional_properties = Some(AdditionalProperties::Schema(Box::new(
                    self.merge_schemas(*left, *right, context)?,
                )));
            }
            (Some(AdditionalProperties::Bool(left)), Some(AdditionalProperties::Bool(right)))
                if left == right =>
            {
                base.additional_properties = Some(AdditionalProperties::Bool(left));
            }
            (Some(value), None) => base.additional_properties = Some(value),
            (None, Some(value)) => base.additional_properties = Some(value),
            (Some(_), Some(_)) => {
                self.handle_unhandled(
                    context,
                    "`allOf` contains incompatible `additionalProperties` declarations",
                )?;
            }
            (None, None) => {}
        }

        let base_properties = base.properties.take();
        let overlay_properties = overlay.properties;
        base.properties = match (base_properties, overlay_properties) {
            (None, None) => None,
            (left, right) => Some(merge_properties(
                self,
                left.unwrap_or_default(),
                right.unwrap_or_default(),
                context,
            )?),
        };

        for (key, value) in overlay.extra_keywords {
            match base.extra_keywords.get(&key) {
                Some(existing) if existing != &value => {
                    if is_known_ignored_schema_keyword(&key) || key.starts_with("x-") {
                        continue;
                    }
                    self.handle_unhandled(
                        context,
                        format!("`allOf` contains incompatible `{key}` declarations"),
                    )?;
                }
                Some(_) => {}
                None => {
                    base.extra_keywords.insert(key, value);
                }
            }
        }

        Ok(base)
    }

    fn import_const_type(
        &mut self,
        schema: &Schema,
        const_value: &Value,
        context: &InlineModelContext,
    ) -> Result<ImportedType> {
        if let Some(schema_type) = schema.primary_schema_type() {
            let imported = match schema_type {
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
                        anyhow!(self.format_context_error(
                            &context.describe(),
                            "array schema is missing `items`",
                            "Add an `items` schema to describe the array element type.",
                        ))
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
            if schema.is_exact_null_type() {
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
        let message = message.into();
        let rendered = self.format_context_error(
            context,
            &message,
            "Use `--ignore-unhandled` to turn this into a warning while keeping generation going.",
        );
        if self.options.ignore_unhandled {
            self.warnings.push(rendered);
            Ok(())
        } else {
            bail!(rendered)
        }
    }

    fn format_context_error(&self, context: &str, message: &str, note: &str) -> String {
        if context.starts_with("#/") {
            self.format_pointer_error(context, message, note)
        } else {
            format!("{context}: {message}\nnote: {note}")
        }
    }

    fn format_pointer_error(&self, pointer: &str, message: &str, note: &str) -> String {
        let mut rendered = format!("OpenAPI document issue\nCaused by:\n  {message}");
        rendered.push_str(&format!("\n  location: {pointer}"));
        if let Some(preview) = self.source.render_pointer_preview(pointer) {
            rendered.push_str("\n  preview:");
            for line in preview.lines() {
                rendered.push_str(&format!("\n    {line}"));
            }
        }
        rendered.push_str(&format!("\n  note: {note}"));
        rendered
    }
}

#[derive(Debug)]
struct LoadedOpenApiDocument {
    document: OpenApiDocument,
    source: OpenApiSource,
}

#[derive(Debug)]
struct OpenApiSource {
    format: SourceFormat,
    value: Value,
}

#[derive(Debug, Clone, Copy)]
enum SourceFormat {
    Json,
    Yaml,
}

impl OpenApiSource {
    fn render_pointer_preview(&self, pointer: &str) -> Option<String> {
        let node = self.value.pointer(pointer.strip_prefix('#').unwrap_or(pointer))?;
        let rendered = match self.format {
            SourceFormat::Json => serde_json::to_string_pretty(node).ok()?,
            SourceFormat::Yaml => serde_yaml::to_string(node).ok()?,
        };
        Some(truncate_preview(&rendered, 10))
    }
}

fn truncate_preview(rendered: &str, max_lines: usize) -> String {
    let lines = rendered.lines().collect::<Vec<_>>();
    if lines.len() <= max_lines {
        return rendered.to_owned();
    }

    let mut output = lines
        .into_iter()
        .take(max_lines)
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    output.push("...".into());
    output.join("\n")
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
    #[serde(default)]
    parameters: BTreeMap<String, ParameterSpec>,
}

#[derive(Debug, Deserialize, Default, Clone)]
struct PathItem {
    #[serde(default)]
    parameters: Option<Vec<ParameterOrRef>>,
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
    parameters: Vec<ParameterOrRef>,
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

#[derive(Debug, Deserialize, Clone)]
#[serde(untagged)]
enum ParameterOrRef {
    Ref {
        #[serde(rename = "$ref")]
        reference: String,
    },
    Inline(ParameterSpec),
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

#[derive(Debug, Deserialize, Default, Clone, PartialEq)]
struct Schema {
    #[serde(rename = "$ref")]
    #[serde(default)]
    reference: Option<String>,
    #[serde(rename = "type")]
    #[serde(default)]
    schema_type: Option<SchemaTypeDecl>,
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
    properties: Option<IndexMap<String, Schema>>,
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

#[derive(Debug, Deserialize, Clone, PartialEq)]
#[serde(untagged)]
enum AdditionalProperties {
    Bool(bool),
    Schema(Box<Schema>),
}

#[derive(Debug, Deserialize, Clone, PartialEq)]
#[serde(untagged)]
enum SchemaTypeDecl {
    Single(String),
    Multiple(Vec<String>),
}

impl SchemaTypeDecl {
    fn as_slice(&self) -> &[String] {
        match self {
            Self::Single(value) => std::slice::from_ref(value),
            Self::Multiple(values) => values.as_slice(),
        }
    }
}

impl Schema {
    fn schema_type_variants(&self) -> Option<&[String]> {
        self.schema_type.as_ref().map(SchemaTypeDecl::as_slice)
    }

    fn primary_schema_type(&self) -> Option<&str> {
        self.schema_type_variants()?
            .iter()
            .find(|value| value.as_str() != "null")
            .map(String::as_str)
    }

    fn is_exact_null_type(&self) -> bool {
        matches!(self.schema_type_variants(), Some([value]) if value == "null")
    }
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
    schema
        .schema_type_variants()
        .is_some_and(|variants| variants.iter().any(|value| value == "object"))
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
        && schema
            .extra_keywords
            .keys()
            .all(|keyword| is_known_ignored_schema_keyword(keyword) || keyword.starts_with("x-"))
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

fn merge_required(mut left: Vec<String>, right: Vec<String>) -> Vec<String> {
    let mut seen = left.iter().cloned().collect::<BTreeSet<_>>();
    for value in right {
        if seen.insert(value.clone()) {
            left.push(value);
        }
    }
    left
}

fn merge_optional_field<T>(
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
                format!("`allOf` contains incompatible `{field_name}` declarations"),
            )?;
        }
    }
    Ok(())
}

fn merge_properties(
    importer: &mut OpenApiImporter,
    mut left: IndexMap<String, Schema>,
    right: IndexMap<String, Schema>,
    context: &str,
) -> Result<IndexMap<String, Schema>> {
    for (key, value) in right {
        if let Some(existing) = left.shift_remove(&key) {
            left.insert(key, importer.merge_schemas(existing, value, context)?);
        } else {
            left.insert(key, value);
        }
    }
    Ok(left)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json_test_source(spec: &str) -> OpenApiSource {
        OpenApiSource {
            format: SourceFormat::Json,
            value: serde_json::from_str(spec).expect("valid json source"),
        }
    }

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
        let result =
            OpenApiImporter::new(document, json_test_source(spec), LoadOpenApiOptions::default())
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
    fn supports_parameter_refs() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {
    "/key/{PK}": {
      "delete": {
        "operationId": "delete_key",
        "parameters": [
          { "$ref": "#/components/parameters/PK" }
        ],
        "responses": {
          "204": { "description": "deleted" }
        }
      }
    }
  },
  "components": {
    "parameters": {
      "PK": {
        "name": "PK",
        "in": "path",
        "required": true,
        "schema": { "type": "string" }
      }
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid test spec");
        let result =
            OpenApiImporter::new(document, json_test_source(spec), LoadOpenApiOptions::default())
                .build_ir()
                .expect("parameter refs should be supported");

        assert_eq!(result.ir.operations.len(), 1);
        let operation = &result.ir.operations[0];
        assert_eq!(operation.params.len(), 1);
        let param = &operation.params[0];
        assert_eq!(param.name, "PK");
        assert_eq!(param.location, ParameterLocation::Path);
        assert!(param.required);
        assert_eq!(param.type_ref, TypeRef::primitive("string"));
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
        let result = OpenApiImporter::new(document, json_test_source(spec), LoadOpenApiOptions::default())
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
    fn supports_type_array_with_nullability() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {},
  "components": {
    "schemas": {
      "Widget": {
        "type": "object",
        "properties": {
          "name": {
            "type": ["string", "null"]
          }
        }
      }
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid test spec");
        let result =
            OpenApiImporter::new(document, json_test_source(spec), LoadOpenApiOptions::default())
                .build_ir()
                .expect("type arrays with null should be supported");
        let widget = result
            .ir
            .models
            .iter()
            .find(|model| model.name == "Widget")
            .expect("Widget model");
        let name = widget
            .fields
            .iter()
            .find(|field| field.name == "name")
            .expect("name field");
        assert_eq!(name.type_ref, TypeRef::primitive("string"));
        assert!(name.nullable);
    }

    #[test]
    fn preserves_schema_property_order() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {},
  "components": {
    "schemas": {
      "Widget": {
        "type": "object",
        "properties": {
          "zebra": { "type": "string" },
          "alpha": { "type": "string" },
          "middle": { "type": "string" }
        }
      }
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid test spec");
        let result =
            OpenApiImporter::new(document, json_test_source(spec), LoadOpenApiOptions::default())
                .build_ir()
                .expect("property order should be preserved");
        let widget = result
            .ir
            .models
            .iter()
            .find(|model| model.name == "Widget")
            .expect("Widget model");
        let field_names = widget
            .fields
            .iter()
            .map(|field| field.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(field_names, vec!["zebra", "alpha", "middle"]);
    }

    #[test]
    fn supports_metadata_only_property_schema_as_any() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {},
  "components": {
    "schemas": {
      "ErrorDetail": {
        "type": "object",
        "properties": {
          "value": {
            "description": "The value at the given location"
          }
        }
      }
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid test spec");
        let result =
            OpenApiImporter::new(document, json_test_source(spec), LoadOpenApiOptions::default())
                .build_ir()
                .expect("metadata-only schema should be treated as any");
        let error_detail = result
            .ir
            .models
            .iter()
            .find(|model| model.name == "ErrorDetail")
            .expect("ErrorDetail model");
        let value = error_detail
            .fields
            .iter()
            .find(|field| field.name == "value")
            .expect("value field");
        assert_eq!(value.type_ref, TypeRef::primitive("any"));
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
        let result = OpenApiImporter::new(document, json_test_source(spec), LoadOpenApiOptions::default())
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
    fn supports_all_of_object_composition() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {},
  "components": {
    "schemas": {
      "Cursor": {
        "type": "object",
        "properties": {
          "cursor": { "type": "string" }
        },
        "required": ["cursor"]
      },
      "PatchSchema": {
        "allOf": [
          { "$ref": "#/components/schemas/Cursor" },
          {
            "type": "object",
            "properties": {
              "items": {
                "type": "array",
                "items": { "type": "string" }
              }
            },
            "required": ["items"]
          }
        ]
      },
      "BaseId": { "type": "string" },
      "WrappedId": {
        "allOf": [
          { "$ref": "#/components/schemas/BaseId" },
          { "description": "Identifier wrapper" }
        ]
      }
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid test spec");
        let result = OpenApiImporter::new(
            document,
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect("allOf should be supported");

        let patch_schema = result
            .ir
            .models
            .iter()
            .find(|model| model.name == "PatchSchema")
            .expect("PatchSchema model");
        let field_names = patch_schema
            .fields
            .iter()
            .map(|field| field.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(field_names, vec!["cursor", "items"]);
        assert!(
            patch_schema
                .fields
                .iter()
                .find(|field| field.name == "cursor")
                .map(|field| !field.optional)
                .unwrap_or(false)
        );
        assert!(
            patch_schema
                .fields
                .iter()
                .find(|field| field.name == "items")
                .map(|field| !field.optional)
                .unwrap_or(false)
        );

        let wrapped_id = result
            .ir
            .models
            .iter()
            .find(|model| model.name == "WrappedId")
            .expect("WrappedId model");
        assert_eq!(
            wrapped_id.attributes.get("alias_type_ref"),
            Some(&json!(TypeRef::primitive("string")))
        );
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn errors_on_recursive_all_of_reference_cycles() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {},
  "components": {
    "schemas": {
      "Node": {
        "allOf": [
          { "$ref": "#/components/schemas/Node" }
        ]
      }
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid test spec");
        let error = OpenApiImporter::new(
            document,
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect_err("recursive allOf cycles should fail cleanly");

        let rendered = format!("{error:#}");
        assert!(
            rendered.contains("recursive reference cycle"),
            "unexpected error: {rendered}"
        );
        assert!(rendered.contains("#/components/schemas/Node"));
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
        "not": {
          "type": "object"
        }
      }
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid test spec");
        let strict_error = OpenApiImporter::new(
            document.clone(),
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect_err("strict mode should fail");
        assert!(strict_error.to_string().contains("`not` is not supported yet"));

        let warning_result = OpenApiImporter::new(
            document,
            json_test_source(spec),
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
                .any(|warning| warning.contains("`not` is not supported yet"))
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
        let error = OpenApiImporter::new(document, json_test_source(spec), LoadOpenApiOptions::default())
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
        let result = OpenApiImporter::new(document, json_test_source(spec), LoadOpenApiOptions::default())
            .build_ir()
            .expect("known ignored keywords should not fail");
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn json_parse_errors_include_schema_path_and_source_context() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {},
  "components": {
    "schemas": {
      "Broken": {
        "type": "object",
        "title": ["not", "a", "string"]
      }
    }
  }
}
"##;

        let error = parse_json_openapi_document(Path::new("broken.json"), spec)
            .expect_err("invalid schema shape should fail during deserialization");
        let message = error.to_string();
        assert!(message.contains("failed to parse JSON OpenAPI document `broken.json`"));
        assert!(message.contains("schema mismatch at `components.schemas.Broken.title`"));
        assert!(message.contains("invalid type"));
        assert!(message.contains("source:         \"title\": [\"not\", \"a\", \"string\"]"));
        assert!(message.contains("note: this usually means"));
    }
}
