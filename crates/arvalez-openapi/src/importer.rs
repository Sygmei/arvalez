use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use anyhow::{Result, anyhow, bail};
use arvalez_ir::{
    Attributes, CoreIr, Field, HttpMethod, Model, Operation, Parameter, RequestBody, Response,
    SourceRef, TypeRef, validate_ir,
};
use indexmap::IndexMap;
use serde_json::{Value, json};

use crate::diagnostic::{DiagnosticKind, OpenApiDiagnostic, OpenApiLoadResult};
use crate::document::{
    AdditionalProperties, OperationSpec, OpenApiDocument,
    ParameterOrRef, ParameterSpec, PathItem, RawParameterLocation, RequestBodyOrRef,
    RequestBodySpec, ResponseSpec, ResponseSpecOrRef, Schema, SchemaOrBool, SchemaTypeDecl,
    raw_parameter_location_label,
};
use crate::merge::{    dedupe_variants, infer_enum_type, infer_format_only_type, infer_schema_type_for_merge,    merge_enum_values, merge_non_codegen_optional_field, merge_optional_field,    merge_properties, merge_required, merge_schema_types,
};
use crate::naming::{
    fallback_operation_name, json_pointer_key, method_key, operation_attributes, to_pascal_case,
    to_snake_case,
};
use crate::schema::{
    decode_json_pointer_segment,
    is_generic_object_placeholder, is_inline_local_schema_reference,
    is_known_but_unimplemented_schema_keyword, is_known_ignored_schema_keyword,
    is_unconstrained_schema, is_validation_only_schema_variant,
    parameter_attributes, ref_name, resolve_nested_schema_reference,
    resolve_response_schema_reference, schema_has_non_all_of_shape, schema_is_object_like,
    schema_runtime_attributes,
};
use crate::source::OpenApiSource;
use crate::{LoadOpenApiOptions, format_duration, measure_openapi_phase};


pub(crate) struct OpenApiImporter {
    pub(crate) document: OpenApiDocument,
    pub(crate) source: OpenApiSource,
    pub(crate) models: BTreeMap<String, Model>,
    pub(crate) generated_model_names: BTreeSet<String>,
    pub(crate) generated_operation_names: BTreeSet<String>,
    pub(crate) local_ref_model_names: BTreeMap<String, String>,
    pub(crate) active_model_builds: BTreeSet<String>,
    pub(crate) active_local_ref_imports: BTreeSet<String>,
    pub(crate) normalized_all_of_refs: BTreeMap<String, Schema>,
    pub(crate) active_all_of_refs: Vec<String>,
    pub(crate) active_object_view_refs: Vec<String>,
    pub(crate) warnings: Vec<OpenApiDiagnostic>,
    pub(crate) options: LoadOpenApiOptions,
}

impl OpenApiImporter {
    pub(crate) fn new(document: OpenApiDocument, source: OpenApiSource, options: LoadOpenApiOptions) -> Self {
        Self {
            document,
            source,
            models: BTreeMap::new(),
            generated_model_names: BTreeSet::new(),
            generated_operation_names: BTreeSet::new(),
            local_ref_model_names: BTreeMap::new(),
            active_model_builds: BTreeSet::new(),
            active_local_ref_imports: BTreeSet::new(),
            normalized_all_of_refs: BTreeMap::new(),
            active_all_of_refs: Vec::new(),
            active_object_view_refs: Vec::new(),
            warnings: Vec::new(),
            options,
        }
    }

    pub(crate) fn build_ir(mut self) -> Result<OpenApiLoadResult> {
        measure_openapi_phase(
            self.options.emit_timings,
            "openapi_component_models",
            || self.import_component_models(),
        )?;

        let mut operations = Vec::new();
        measure_openapi_phase(self.options.emit_timings, "openapi_operations", || {
            let paths = self.document.paths.clone();
            for (path, item) in &paths {
                operations.extend(self.import_path_item(path, item)?);
            }
            Ok(())
        })?;

        let ir = CoreIr {
            models: self.models.into_values().collect(),
            operations,
            ..Default::default()
        };

        measure_openapi_phase(self.options.emit_timings, "openapi_validate_ir", || {
            validate_ir(&ir).map_err(|errors| {
                let details = errors
                    .0
                    .iter()
                    .map(|issue| format!("{}: {}", issue.path, issue.message))
                    .collect::<Vec<_>>()
                    .join("\n");
                anyhow!("generated IR is invalid:\n{details}")
            })
        })?;
        Ok(OpenApiLoadResult {
            ir,
            warnings: self.warnings,
        })
    }

    fn import_component_models(&mut self) -> Result<()> {
        let mut schemas = Vec::new();
        for (name, schema) in self.document.components.schemas.clone() {
            let pointer = format!("#/components/schemas/{name}");
            schemas.push((name, schema, pointer));
        }
        for (name, schema) in self.document.definitions.clone() {
            let pointer = format!("#/definitions/{name}");
            schemas.push((name, schema, pointer));
        }
        let total = schemas.len();
        for (index, (name, schema, pointer)) in schemas.into_iter().enumerate() {
            if self.options.emit_timings {
                eprintln!(
                    "timing: starting component_model [{}/{}] {}",
                    index + 1,
                    total,
                    name
                );
            }
            let started = Instant::now();
            self.ensure_named_schema_model(&name, &schema, &pointer)?;
            if self.options.emit_timings {
                eprintln!(
                    "timing: component_model [{}/{}] {:<40} {}",
                    index + 1,
                    total,
                    name,
                    format_duration(started.elapsed())
                );
            }
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

            let operation_name = self.reserve_operation_name(
                spec.operation_id
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned)
                    .unwrap_or_else(|| fallback_operation_name(method, path)),
            );
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
            let mut unnamed_parameter_counter = 0usize;
            let mut form_data_parameters = Vec::new();
            let shared_len = shared_parameters.len();

            for (param_idx, param) in shared_parameters.iter().chain(spec.parameters.iter()).enumerate() {
                let mut resolved = self.resolve_parameter(param)?;
                if resolved.name.trim().is_empty() {
                    unnamed_parameter_counter += 1;
                    // Use the specific parameter pointer so the source preview
                    // and line number point at the offending parameter item
                    // rather than the whole operation.
                    let param_pointer = if param_idx < shared_len {
                        format!("#/paths/{}/parameters/{}", json_pointer_key(path), param_idx)
                    } else {
                        format!(
                            "#/paths/{}/{}/parameters/{}",
                            json_pointer_key(path),
                            method_key(method),
                            param_idx - shared_len,
                        )
                    };
                    self.handle_unhandled(
                        &param_pointer,
                        DiagnosticKind::EmptyParameterName {
                            counter: unnamed_parameter_counter,
                        },
                    )?;
                    resolved.name = format!(
                        "unnamed_{}_parameter_{}",
                        raw_parameter_location_label(resolved.location),
                        unnamed_parameter_counter
                    );
                }
                if resolved.location == RawParameterLocation::Body {
                    let request_body =
                        self.import_swagger_body_parameter(&resolved, spec, &operation_name)?;
                    if operation.request_body.is_some() {
                        bail!(self.make_diagnostic(
                            &format!("operation `{operation_name}`"),
                            DiagnosticKind::MultipleRequestBodyDeclarations {
                                note: "Arvalez can normalize either an OpenAPI `requestBody` or a single Swagger 2 `in: body` parameter for an operation.".into(),
                            },
                        ));
                    }
                    operation.request_body = Some(request_body);
                    continue;
                }
                if resolved.location == RawParameterLocation::FormData {
                    form_data_parameters.push(resolved);
                    continue;
                }

                operation.params.push(self.import_parameter(&resolved)?);
            }

            if !form_data_parameters.is_empty() {
                if operation.request_body.is_some() {
                    bail!(self.make_diagnostic(
                        &format!("operation `{operation_name}`"),
                        DiagnosticKind::MultipleRequestBodyDeclarations {
                            note: "Arvalez can normalize either an OpenAPI `requestBody`, a single Swagger 2 `in: body` parameter, or Swagger 2 `formData` parameters for an operation.".into(),
                        },
                    ));
                }
                operation.request_body = Some(self.import_swagger_form_data_request_body(
                    &form_data_parameters,
                    spec,
                    &operation_name,
                )?);
            }

            if let Some(request_body) = &spec.request_body {
                if operation.request_body.is_some() {
                    bail!(self.make_diagnostic(
                        &format!("operation `{operation_name}`"),
                        DiagnosticKind::MultipleRequestBodyDeclarations {
                            note: "Arvalez can normalize either an OpenAPI `requestBody` or a single Swagger 2 `in: body` parameter for an operation.".into(),
                        },
                    ));
                }
                operation.request_body =
                    Some(self.import_request_body(request_body, &operation_name, path, method)?);
            }

            for (status, response_or_ref) in &spec.responses {
                let response = self.resolve_response_spec(response_or_ref)?;
                operation.responses.push(self.import_response(
                    status,
                    &response,
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
        let schema = param.effective_schema().ok_or_else(|| {
            anyhow::Error::new(self.make_diagnostic(
                &format!("parameter `{}`", param.name),
                DiagnosticKind::ParameterMissingSchema {
                    name: param.name.clone(),
                },
            ))
        })?;
        let imported = self.import_schema_type(
            &schema,
            &InlineModelContext::Parameter {
                name: param.name.clone(),
            },
        )?;

        Ok(Parameter {
            name: param.name.clone(),
            location: param.location.as_ir_location().ok_or_else(|| {
                anyhow::Error::new(self.make_diagnostic(
                    &format!("parameter `{}`", param.name),
                    DiagnosticKind::UnsupportedParameterLocation {
                        name: param.name.clone(),
                    },
                ))
            })?,
            type_ref: imported
                .type_ref
                .unwrap_or_else(|| TypeRef::primitive("any")),
            required: param.required,
            attributes: parameter_attributes(&param, &schema),
        })
    }

    fn import_swagger_body_parameter(
        &mut self,
        param: &ParameterSpec,
        spec: &OperationSpec,
        operation_name: &str,
    ) -> Result<RequestBody> {
        let schema = param.effective_schema().ok_or_else(|| {
            anyhow::Error::new(self.make_diagnostic(
                &format!("body parameter `{}`", param.name),
                DiagnosticKind::BodyParameterMissingSchema {
                    name: param.name.clone(),
                },
            ))
        })?;

        let imported = self.import_schema_type(
            &schema,
            &InlineModelContext::RequestBody {
                operation_name: operation_name.to_owned(),
                pointer: format!(
                    "#/operations/{operation_name}/body_parameter/{}",
                    param.name
                ),
            },
        )?;

        let media_type = spec
            .consumes
            .first()
            .cloned()
            .or_else(|| self.document.consumes.first().cloned())
            .unwrap_or_else(|| "application/json".into());

        let mut attributes = schema_runtime_attributes(&schema);
        if !param.description.trim().is_empty() {
            attributes.insert(
                "description".into(),
                Value::String(param.description.trim().to_owned()),
            );
        }

        Ok(RequestBody {
            required: param.required,
            media_type,
            type_ref: imported.type_ref,
            attributes,
        })
    }

    fn import_swagger_form_data_request_body(
        &mut self,
        params: &[ParameterSpec],
        spec: &OperationSpec,
        operation_name: &str,
    ) -> Result<RequestBody> {
        let mut properties = IndexMap::new();
        let mut required = Vec::new();
        for param in params {
            let mut schema = param.effective_schema().ok_or_else(|| {
                anyhow::Error::new(self.make_diagnostic(
                    &format!("formData parameter `{}`", param.name),
                    DiagnosticKind::FormDataParameterMissingSchema {
                        name: param.name.clone(),
                    },
                ))
            })?;
            if !param.description.trim().is_empty() {
                schema.extra_keywords.insert(
                    "description".into(),
                    Value::String(param.description.trim().to_owned()),
                );
            }
            if param.required {
                required.push(param.name.clone());
            }
            properties.insert(param.name.clone(), SchemaOrBool::Schema(schema));
        }

        let imported = self.import_schema_type(
            &Schema {
                schema_type: Some(SchemaTypeDecl::Single("object".into())),
                properties: Some(properties),
                required: (!required.is_empty()).then_some(required.clone()),
                ..Schema::default()
            },
            &InlineModelContext::RequestBody {
                operation_name: operation_name.to_owned(),
                pointer: format!("#/operations/{operation_name}/formData"),
            },
        )?;

        let media_type = spec
            .consumes
            .first()
            .cloned()
            .or_else(|| self.document.consumes.first().cloned())
            .unwrap_or_else(|| "application/x-www-form-urlencoded".into());

        let mut attributes = Attributes::default();
        if params.iter().any(|param| param.required) {
            attributes.insert("form_encoding".into(), Value::String(media_type.clone()));
        }

        Ok(RequestBody {
            required: params.iter().any(|param| param.required),
            media_type,
            type_ref: imported.type_ref,
            attributes,
        })
    }

    fn resolve_parameter(&self, param: &ParameterOrRef) -> Result<ParameterSpec> {
        let mut seen = BTreeSet::new();
        self.resolve_parameter_inner(param, &mut seen)
    }

    fn resolve_parameter_inner(
        &self,
        param: &ParameterOrRef,
        seen: &mut BTreeSet<String>,
    ) -> Result<ParameterSpec> {
        match param {
            ParameterOrRef::Inline(param) => Ok(param.clone()),
            ParameterOrRef::Ref { reference } => {
                if !seen.insert(reference.clone()) {
                    bail!(self.make_pointer_diagnostic(
                        reference,
                        DiagnosticKind::RecursiveParameterCycle {
                            reference: reference.to_owned()
                        },
                    ));
                }

                if let Some(parameter) = self.resolve_named_parameter_reference(reference) {
                    return self
                        .resolve_parameter_inner(&ParameterOrRef::Inline(parameter.clone()), seen);
                }

                if let Some(parameter) = self.resolve_path_parameter_reference(reference)? {
                    return self.resolve_parameter_inner(parameter, seen);
                }

                Err(anyhow!("unsupported reference `{reference}`"))
            }
        }
    }

    fn resolve_named_parameter_reference(&self, reference: &str) -> Option<&ParameterSpec> {
        let name = ref_name(reference).ok()?;
        self.document
            .components
            .parameters
            .get(&name)
            .or_else(|| self.document.parameters.get(&name))
    }

    fn resolve_path_parameter_reference<'a>(
        &'a self,
        reference: &str,
    ) -> Result<Option<&'a ParameterOrRef>> {
        let Some(pointer) = reference.strip_prefix("#/") else {
            return Ok(None);
        };
        let segments = pointer
            .split('/')
            .map(decode_json_pointer_segment)
            .collect::<Result<Vec<_>>>()?;
        if segments.first().map(String::as_str) != Some("paths") {
            return Ok(None);
        }

        match segments.as_slice() {
            [_, path, scope, index] if scope == "parameters" => {
                let index = index.parse::<usize>().ok();
                let param = self
                    .document
                    .paths
                    .get(path)
                    .and_then(|item| item.parameters.as_ref())
                    .and_then(|params| index.and_then(|idx| params.get(idx)));
                Ok(param)
            }
            [_, path, method, scope, index] if scope == "parameters" => {
                let index = index.parse::<usize>().ok();
                let param = self
                    .document
                    .paths
                    .get(path)
                    .and_then(|item| match method.as_str() {
                        "get" => item.get.as_ref(),
                        "post" => item.post.as_ref(),
                        "put" => item.put.as_ref(),
                        "patch" => item.patch.as_ref(),
                        "delete" => item.delete.as_ref(),
                        _ => None,
                    })
                    .and_then(|operation| index.and_then(|idx| operation.parameters.get(idx)));
                Ok(param)
            }
            _ => Ok(None),
        }
    }

    fn import_request_body(
        &mut self,
        request_body: &RequestBodyOrRef,
        operation_name: &str,
        path: &str,
        method: HttpMethod,
    ) -> Result<RequestBody> {
        let fallback_pointer = format!(
            "#/paths/{}/{}/requestBody",
            json_pointer_key(path),
            method_key(method)
        );
        let (request_body, pointer) = self.resolve_request_body(request_body, &fallback_pointer)?;
        let content_pointer = format!("{pointer}/content");
        let Some((media_type, media_spec)) = request_body.content.iter().next() else {
            self.warnings.push(self.make_pointer_diagnostic(
                &content_pointer,
                DiagnosticKind::EmptyRequestBodyContent,
            ));
            return Ok(RequestBody {
                required: request_body.required,
                media_type: "application/octet-stream".into(),
                type_ref: None,
                attributes: Attributes::default(),
            });
        };

        let imported = media_spec
            .schema
            .as_ref()
            .map(|schema| {
                self.import_schema_type(
                    schema,
                    &InlineModelContext::RequestBody {
                        operation_name: operation_name.to_owned(),
                        pointer: format!(
                            "{content_pointer}/{}/schema",
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
            attributes: media_spec
                .schema
                .as_ref()
                .map(schema_runtime_attributes)
                .unwrap_or_default(),
        })
    }

    fn resolve_request_body(
        &self,
        request_body: &RequestBodyOrRef,
        pointer: &str,
    ) -> Result<(RequestBodySpec, String)> {
        let mut seen = BTreeSet::new();
        self.resolve_request_body_inner(request_body, pointer, &mut seen)
    }

    fn resolve_request_body_inner(
        &self,
        request_body: &RequestBodyOrRef,
        pointer: &str,
        seen: &mut BTreeSet<String>,
    ) -> Result<(RequestBodySpec, String)> {
        match request_body {
            RequestBodyOrRef::Inline(spec) => Ok((spec.clone(), pointer.to_owned())),
            RequestBodyOrRef::Ref { reference } => {
                if !seen.insert(reference.clone()) {
                    bail!(self.make_pointer_diagnostic(
                        reference,
                        DiagnosticKind::RecursiveRequestBodyCycle {
                            reference: reference.to_owned()
                        },
                    ));
                }
                let name = ref_name(reference)?;
                let referenced = self
                    .document
                    .components
                    .request_bodies
                    .get(&name)
                    .ok_or_else(|| anyhow!("unsupported reference `{reference}`"))?;
                self.resolve_request_body_inner(referenced, reference, seen)
            }
        }
    }

    fn resolve_response_spec(&self, response: &ResponseSpecOrRef) -> Result<ResponseSpec> {
        match &response.reference {
            None => Ok(ResponseSpec {
                description: response.description.clone(),
                content: response.content.clone(),
            }),
            Some(reference) => {
                let Some(pointer) = reference.strip_prefix("#/") else {
                    return Err(anyhow!("unsupported reference `{reference}`"));
                };
                let segments: Vec<&str> = pointer.split('/').collect();
                match segments.as_slice() {
                    // OpenAPI 3: #/components/responses/{name}
                    ["components", "responses", name] => self
                        .document
                        .components
                        .responses
                        .get(*name)
                        .cloned()
                        .ok_or_else(|| anyhow!("unsupported reference `{reference}`")),
                    // Swagger 2: #/responses/{name}
                    ["responses", name] => self
                        .document
                        .responses
                        .get(*name)
                        .cloned()
                        .ok_or_else(|| anyhow!("unsupported reference `{reference}`")),
                    // Inline path reference (e.g. #/paths/.../responses/200) — return
                    // empty response, preserving the same silent-fallback behaviour that
                    // existed before ResponseSpecOrRef was introduced.
                    _ => Ok(ResponseSpec::default()),
                }
            }
        }
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
                            },
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
        if let Some(schema) = schema {
            attributes.extend(schema_runtime_attributes(schema));
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
        if schema.all_of.is_some() && schema_is_object_like(schema) {
            return self.build_object_model_from_all_of(name, schema, pointer);
        }

        let schema = self.normalize_schema(schema, pointer)?;
        let schema = schema.as_ref();
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
            let schema_type = schema.primary_schema_type().unwrap_or("string");
            model.attributes.insert(
                "enum_base_type".into(),
                Value::String(schema_type.to_owned()),
            );
            if let Some(title) = &schema.title {
                model
                    .attributes
                    .insert("title".into(), Value::String(title.clone()));
            }
            return Ok(model);
        }

        if !schema_is_object_like(schema) {
            let imported = self.import_schema_type_normalized(
                schema,
                &InlineModelContext::NamedSchema {
                    name: name.to_owned(),
                    pointer: pointer.to_owned(),
                },
                true,
                None,
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
            model.attributes.extend(schema_runtime_attributes(schema));
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

        let mut unnamed_field_counter = 0usize;
        for (field_name, property_schema_or_bool) in properties {
            // Boolean schemas (OpenAPI 3.1: `false`/`true`) have no codegen meaning — skip.
            let Some(property_schema) = property_schema_or_bool.as_schema() else {
                continue;
            };
            let original_field_name = field_name.clone();
            let field_name = self.normalize_field_name(
                field_name.clone(),
                &format!("{pointer}/properties"),
                &mut unnamed_field_counter,
            )?;
            let imported = self.import_schema_type(
                property_schema,
                &InlineModelContext::Field {
                    model_name: name.to_owned(),
                    field_name: original_field_name.clone(),
                    pointer: format!(
                        "{}/properties/{}",
                        pointer,
                        json_pointer_key(&original_field_name)
                    ),
                },
            )?;
            let mut field = Field::new(
                field_name.clone(),
                imported
                    .type_ref
                    .unwrap_or_else(|| TypeRef::primitive("any")),
            );
            field.optional = !required.contains(original_field_name.as_str());
            field.nullable = imported.nullable;
            field.attributes = schema_runtime_attributes(property_schema);
            model.fields.push(field);
        }

        Ok(model)
    }

    fn build_object_model_from_all_of(
        &mut self,
        name: &str,
        schema: &Schema,
        pointer: &str,
    ) -> Result<Model> {
        let view = self.collect_object_schema_view(schema, pointer)?;
        let required = view.required;

        let mut model = Model::new(format!("model.{}", to_snake_case(name)), name.to_owned());
        model.source = Some(SourceRef {
            pointer: pointer.to_owned(),
            line: None,
        });
        if let Some(title) = view.title {
            model
                .attributes
                .insert("title".into(), Value::String(title));
        }

        let mut unnamed_field_counter = 0usize;
        for (field_name, property_schema) in view.properties {
            let original_field_name = field_name.clone();
            let field_name = self.normalize_field_name(
                field_name,
                &format!("{pointer}/properties"),
                &mut unnamed_field_counter,
            )?;
            let imported = self.import_schema_type(
                &property_schema,
                &InlineModelContext::Field {
                    model_name: name.to_owned(),
                    field_name: original_field_name.clone(),
                    pointer: format!(
                        "{}/properties/{}",
                        pointer,
                        json_pointer_key(&original_field_name)
                    ),
                },
            )?;
            let mut field = Field::new(
                field_name.clone(),
                imported
                    .type_ref
                    .unwrap_or_else(|| TypeRef::primitive("any")),
            );
            field.optional = !required.contains(original_field_name.as_str());
            field.nullable = imported.nullable;
            field.attributes = schema_runtime_attributes(&property_schema);
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
        let local_reference = schema
            .reference
            .as_deref()
            .filter(|reference| is_inline_local_schema_reference(reference))
            .map(ToOwned::to_owned);
        if let Some(imported) = self.import_decorated_reference_type(schema, context)? {
            return Ok(imported);
        }
        let schema = self.normalize_schema(schema, &context.describe())?;
        self.import_schema_type_normalized(
            schema.as_ref(),
            context,
            skip_keyword_validation,
            local_reference.as_deref(),
        )
    }

    fn import_decorated_reference_type(
        &mut self,
        schema: &Schema,
        context: &InlineModelContext,
    ) -> Result<Option<ImportedType>> {
        if matches!(context, InlineModelContext::NamedSchema { .. }) {
            return Ok(None);
        }

        let all_of = match &schema.all_of {
            Some(all_of) => all_of,
            None => return Ok(None),
        };

        if schema_has_non_all_of_shape(schema) {
            return Ok(None);
        }

        let mut reference: Option<&str> = None;
        for member in all_of {
            if let Some(member_ref) = member.reference.as_deref() {
                if reference.replace(member_ref).is_some() {
                    return Ok(None);
                }
                continue;
            }

            if !is_unconstrained_schema(member) {
                return Ok(None);
            }
        }

        let Some(reference) = reference else {
            return Ok(None);
        };

        Ok(Some(ImportedType {
            type_ref: Some(TypeRef::named(ref_name(reference)?)),
            nullable: false,
        }))
    }

    fn import_schema_type_normalized(
        &mut self,
        schema: &Schema,
        context: &InlineModelContext,
        skip_keyword_validation: bool,
        local_reference: Option<&str>,
    ) -> Result<ImportedType> {
        if !skip_keyword_validation {
            self.validate_schema_keywords(schema, &context.describe())?;
        }

        if let Some(reference) = &schema.reference {
            if is_inline_local_schema_reference(reference) {
                if self.active_local_ref_imports.contains(reference) {
                    let model_name = self
                        .local_ref_model_names
                        .get(reference)
                        .cloned()
                        .unwrap_or_else(|| {
                            to_pascal_case(
                                &ref_name(reference).unwrap_or_else(|_| "RecursiveModel".into()),
                            )
                        });
                    return Ok(ImportedType::plain(TypeRef::named(model_name)));
                }

                self.active_local_ref_imports.insert(reference.clone());
                // Use the cycle-safe resolve+expand path so that allOf schemas
                // (e.g. `{allOf: [{$ref: "..."}]}`) are fully shaped before
                // import_schema_type_normalized sees them, without risking
                // infinite recursion on self-referential schemas.
                let resolved =
                    self.resolve_schema_reference_for_all_of(reference, &context.describe())?;
                if schema_is_object_like(&resolved) {
                    let already_registered = self.local_ref_model_names.contains_key(reference);
                    if !already_registered {
                        let model_name = self.inline_model_name(&resolved, context);
                        self.local_ref_model_names
                            .insert(reference.clone(), model_name);
                    } else {
                        // Model was already imported from a previous (non-recursive) call site.
                        // Return a named reference immediately to avoid re-processing the full
                        // expanded schema from each call site, which would create exponentially
                        // many inline models for mutually-referential schemas (e.g. Azure specs).
                        let model_name = self.local_ref_model_names[reference].clone();
                        self.active_local_ref_imports.remove(reference);
                        return Ok(ImportedType::plain(TypeRef::named(model_name)));
                    }
                }
                let result = self.import_schema_type_normalized(
                    &resolved,
                    context,
                    skip_keyword_validation,
                    Some(reference),
                );
                self.active_local_ref_imports.remove(reference);
                return result;
            }
            return Ok(ImportedType {
                type_ref: Some(TypeRef::named(ref_name(reference)?)),
                nullable: false,
            });
        }

        if let Some(const_value) = &schema.const_value {
            return self.import_const_type(&schema, const_value, context);
        }

        if schema_is_object_like(schema)
            && schema
                .any_of
                .as_ref()
                .is_some_and(|variants| variants.iter().all(is_validation_only_schema_variant))
        {
            return self.import_object_type(schema, context, local_reference);
        }

        if let Some(any_of) = &schema.any_of {
            return self.import_any_of(any_of, context);
        }

        if schema_is_object_like(schema)
            && schema
                .one_of
                .as_ref()
                .is_some_and(|variants| variants.iter().all(is_validation_only_schema_variant))
        {
            return self.import_object_type(schema, context, local_reference);
        }

        if let Some(one_of) = &schema.one_of {
            return self.import_any_of(one_of, context);
        }

        if let Some(imported) = self.import_implicit_schema_type(schema, context)? {
            return Ok(imported);
        }

        if let Some(imported) = self.import_schema_type_from_decl(&schema, context)? {
            return Ok(imported);
        }

        if is_unconstrained_schema(&schema) {
            return Ok(ImportedType::plain(TypeRef::primitive("any")));
        }

        if schema.properties.is_some() || schema.additional_properties.is_some() {
            return self.import_object_type(&schema, context, local_reference);
        }

        self.handle_unhandled(&context.describe(), DiagnosticKind::UnsupportedSchemaShape)?;
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

        if let Some(embedded) = schema_types.embedded_schema() {
            return Ok(Some(self.import_schema_type(embedded, context)?));
        }

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
                    match schema.items.as_ref() {
                        Some(item_schema) => {
                            let imported = self.import_schema_type(item_schema, context)?;
                            ImportedType::plain(TypeRef::array(
                                imported
                                    .type_ref
                                    .unwrap_or_else(|| TypeRef::primitive("any")),
                            ))
                        }
                        // JSON Schema: array without `items` means array of any.
                        None => ImportedType::plain(TypeRef::array(TypeRef::primitive("any"))),
                    }
                }
                "object" => self.import_object_type(schema, context, None)?,
                "file" => ImportedType::plain(TypeRef::primitive("binary")),
                "null" => ImportedType {
                    type_ref: Some(TypeRef::primitive("any")),
                    nullable: true,
                },
                other => {
                    self.handle_unhandled(
                        &context.describe(),
                        DiagnosticKind::UnsupportedSchemaType {
                            schema_type: other.to_owned(),
                        },
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
            _ => Some(TypeRef::Union {
                variants: type_refs,
            }),
        };

        Ok(Some(ImportedType { type_ref, nullable }))
    }

    fn import_implicit_schema_type(
        &mut self,
        schema: &Schema,
        context: &InlineModelContext,
    ) -> Result<Option<ImportedType>> {
        if let Some(enum_values) = &schema.enum_values {
            let inferred = infer_enum_type(enum_values, schema.format.as_deref());
            return Ok(Some(ImportedType {
                type_ref: Some(inferred),
                nullable: false,
            }));
        }

        if schema.items.is_some() {
            let item_schema = schema.items.as_ref().expect("checked is_some");
            let imported = self.import_schema_type(item_schema, context)?;
            return Ok(Some(ImportedType::plain(TypeRef::array(
                imported
                    .type_ref
                    .unwrap_or_else(|| TypeRef::primitive("any")),
            ))));
        }

        if let Some(type_ref) = infer_format_only_type(schema.format.as_deref()) {
            return Ok(Some(ImportedType::plain(type_ref)));
        }

        // Format present but unrecognized by type inference (e.g. a human-readable
        // sentence used as the format value). Treat the schema as unconstrained
        // rather than failing with an unsupported-shape error.
        if schema.format.is_some() {
            return Ok(Some(ImportedType::plain(TypeRef::primitive("any"))));
        }

        Ok(None)
    }

    fn validate_schema_keywords(&mut self, schema: &Schema, context: &str) -> Result<()> {
        for keyword in schema.extra_keywords.keys() {
            if is_known_ignored_schema_keyword(keyword) || keyword.starts_with("x-") {
                continue;
            }

            if is_known_but_unimplemented_schema_keyword(keyword) {
                self.handle_unhandled(
                    context,
                    DiagnosticKind::UnsupportedSchemaKeyword {
                        keyword: keyword.clone(),
                    },
                )?;
                continue;
            }

            self.handle_unhandled(
                context,
                DiagnosticKind::UnknownSchemaKeyword {
                    keyword: keyword.clone(),
                },
            )?;
        }

        Ok(())
    }

    fn normalize_schema<'a>(
        &mut self,
        schema: &'a Schema,
        context: &str,
    ) -> Result<Cow<'a, Schema>> {
        if schema.all_of.is_none() {
            return Ok(Cow::Borrowed(schema));
        }

        let normalized = self.expand_all_of_schema(schema, context)?;
        Ok(Cow::Owned(normalized))
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
                DiagnosticKind::AllOfRecursiveCycle {
                    reference: reference.to_owned(),
                },
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
        let Some(pointer) = reference.strip_prefix("#/") else {
            bail!("unsupported reference `{reference}`");
        };
        let segments = pointer
            .split('/')
            .map(decode_json_pointer_segment)
            .collect::<Result<Vec<_>>>()?;
        enum ResolvedSchemaRef<'a> {
            Borrowed(&'a Schema),
            Owned(Schema),
        }

        let (resolved, remainder): (ResolvedSchemaRef<'_>, &[String]) = match segments.as_slice() {
            [root, collection, name, rest @ ..]
                if root == "components" && collection == "schemas" =>
            {
                (
                    ResolvedSchemaRef::Borrowed(
                        self.document
                            .components
                            .schemas
                            .get(name)
                            .ok_or_else(|| anyhow!("unsupported reference `{reference}`"))?,
                    ),
                    rest,
                )
            }
            [root, name, rest @ ..] if root == "definitions" => (
                ResolvedSchemaRef::Borrowed(
                    self.document
                        .definitions
                        .get(name)
                        .ok_or_else(|| anyhow!("unsupported reference `{reference}`"))?,
                ),
                rest,
            ),
            [root, collection, name, schema_segment, rest @ ..]
                if root == "components"
                    && collection == "parameters"
                    && schema_segment == "schema" =>
            {
                (
                    ResolvedSchemaRef::Owned(
                        self.document
                            .components
                            .parameters
                            .get(name)
                            .and_then(ParameterSpec::effective_schema)
                            .ok_or_else(|| anyhow!("unsupported reference `{reference}`"))?,
                    ),
                    rest,
                )
            }
            [root, name, schema_segment, rest @ ..]
                if root == "parameters" && schema_segment == "schema" =>
            {
                (
                    ResolvedSchemaRef::Owned(
                        self.document
                            .parameters
                            .get(name)
                            .and_then(ParameterSpec::effective_schema)
                            .ok_or_else(|| anyhow!("unsupported reference `{reference}`"))?,
                    ),
                    rest,
                )
            }
            // #/components/responses/{name} — use the first available schema
            [root, collection, name, rest @ ..]
                if root == "components" && collection == "responses" =>
            {
                let response = self
                    .document
                    .components
                    .responses
                    .get(name)
                    .ok_or_else(|| anyhow!("unsupported reference `{reference}`"))?;
                // `rest` may continue into content/{media_type}/schema/...
                // Resolve via the helper that understands response continuation.
                return resolve_response_schema_reference(response, rest, reference);
            }
            // #/paths/{path}/{method}/responses/{status}/content/{media}/schema
            // #/paths/{path}/{method}/responses/{status}
            [root, path, method, responses_key, status, rest @ ..]
                if root == "paths" && responses_key == "responses" =>
            {
                let operation = self
                    .document
                    .paths
                    .get(path)
                    .and_then(|item| match method.as_str() {
                        "get" => item.get.as_ref(),
                        "post" => item.post.as_ref(),
                        "put" => item.put.as_ref(),
                        "patch" => item.patch.as_ref(),
                        "delete" => item.delete.as_ref(),
                        _ => None,
                    })
                    .ok_or_else(|| anyhow!("unsupported reference `{reference}`"))?;
                let response_or_ref = operation
                    .responses
                    .get(status)
                    .ok_or_else(|| anyhow!("unsupported reference `{reference}`"))?;
                let response = self.resolve_response_spec(response_or_ref)?;
                return resolve_response_schema_reference(&response, rest, reference);
            }
            // #/paths/{path}/{method}/requestBody/content/{media_type}/schema/...
            [
                root,
                path,
                method,
                rb_key,
                content_key,
                media_type,
                schema_key,
                rest @ ..,
            ] if root == "paths"
                && rb_key == "requestBody"
                && content_key == "content"
                && schema_key == "schema" =>
            {
                let operation = self
                    .document
                    .paths
                    .get(path)
                    .and_then(|item| match method.as_str() {
                        "get" => item.get.as_ref(),
                        "post" => item.post.as_ref(),
                        "put" => item.put.as_ref(),
                        "patch" => item.patch.as_ref(),
                        "delete" => item.delete.as_ref(),
                        _ => None,
                    })
                    .ok_or_else(|| anyhow!("unsupported reference `{reference}`"))?;
                let request_body = match operation.request_body.as_ref() {
                    Some(RequestBodyOrRef::Inline(rb)) => rb,
                    _ => bail!("unsupported reference `{reference}`"),
                };
                let schema = request_body
                    .content
                    .get(media_type.as_str())
                    .and_then(|m| m.schema.as_ref())
                    .ok_or_else(|| anyhow!("unsupported reference `{reference}`"))?;
                return resolve_nested_schema_reference(schema, rest, reference);
            }
            // #/paths/{path}/{method}/parameters/{index}/schema/...
            [
                root,
                path,
                method,
                params_key,
                index_str,
                schema_key,
                rest @ ..,
            ] if root == "paths" && params_key == "parameters" && schema_key == "schema" => {
                let idx: usize = index_str
                    .parse()
                    .map_err(|_| anyhow!("unsupported reference `{reference}`"))?;
                let path_item = self
                    .document
                    .paths
                    .get(path)
                    .ok_or_else(|| anyhow!("unsupported reference `{reference}`"))?;
                let operation = match method.as_str() {
                    "get" => path_item.get.as_ref(),
                    "post" => path_item.post.as_ref(),
                    "put" => path_item.put.as_ref(),
                    "patch" => path_item.patch.as_ref(),
                    "delete" => path_item.delete.as_ref(),
                    _ => None,
                }
                .ok_or_else(|| anyhow!("unsupported reference `{reference}`"))?;
                let param_spec = operation
                    .parameters
                    .get(idx)
                    .or_else(|| {
                        path_item
                            .parameters
                            .as_ref()
                            .and_then(|params| params.get(idx))
                    })
                    .and_then(|p| match p {
                        ParameterOrRef::Inline(spec) => Some(spec),
                        _ => None,
                    })
                    .ok_or_else(|| anyhow!("unsupported reference `{reference}`"))?;
                let schema = param_spec
                    .effective_schema()
                    .ok_or_else(|| anyhow!("unsupported reference `{reference}`"))?;
                return resolve_nested_schema_reference(&schema, rest, reference);
            }
            _ => bail!("unsupported reference `{reference}`"),
        };

        let schema = match &resolved {
            ResolvedSchemaRef::Borrowed(schema) => *schema,
            ResolvedSchemaRef::Owned(schema) => schema,
        };
        resolve_nested_schema_reference(schema, remainder, reference)
    }

    fn reserve_operation_name(&mut self, base: String) -> String {
        if self.generated_operation_names.insert(base.clone()) {
            return base;
        }

        let mut counter = 2usize;
        loop {
            let candidate = format!("{base}_{counter}");
            if self.generated_operation_names.insert(candidate.clone()) {
                return candidate;
            }
            counter += 1;
        }
    }

    fn normalize_field_name(
        &mut self,
        field_name: String,
        context: &str,
        unnamed_field_counter: &mut usize,
    ) -> Result<String> {
        if !field_name.trim().is_empty() {
            return Ok(field_name);
        }

        *unnamed_field_counter += 1;
        // Point at the specific empty-string key within the properties mapping so
        // the source preview and line number resolve to the offending `"":` entry
        // rather than the `properties:` block as a whole.
        let specific_pointer = format!("{}/{}", context, json_pointer_key(&field_name));
        self.handle_unhandled(
            &specific_pointer,
            DiagnosticKind::EmptyPropertyKey {
                counter: *unnamed_field_counter,
            },
        )?;
        Ok(format!("unnamed_field_{}", unnamed_field_counter))
    }

    pub(crate) fn merge_schemas(
        &mut self,
        mut base: Schema,
        overlay: Schema,
        context: &str,
    ) -> Result<Schema> {
        let inferred_base_type = infer_schema_type_for_merge(&base);
        let inferred_overlay_type = infer_schema_type_for_merge(&overlay);
        let base_is_generic_object_placeholder = is_generic_object_placeholder(&base);
        let overlay_is_generic_object_placeholder = is_generic_object_placeholder(&overlay);
        let base_schema_type = base.schema_type.take();
        let overlay_schema_type = overlay.schema_type.clone();
        merge_non_codegen_optional_field(&mut base.definitions, overlay.definitions);
        merge_non_codegen_optional_field(&mut base.title, overlay.title);
        merge_non_codegen_optional_field(&mut base.format, overlay.format);
        base.schema_type = merge_schema_types(
            inferred_base_type,
            inferred_overlay_type,
            base_is_generic_object_placeholder,
            overlay_is_generic_object_placeholder,
            base_schema_type,
            overlay_schema_type,
            context,
            self,
        )?;
        merge_optional_field(
            &mut base.const_value,
            overlay.const_value,
            "const",
            context,
            self,
        )?;
        merge_non_codegen_optional_field(&mut base._discriminator, overlay._discriminator);
        base.enum_values =
            merge_enum_values(base.enum_values.take(), overlay.enum_values, context, self)?;
        // incompatible anyOf/oneOf in allOf — keep the base side rather than erroring.
        merge_non_codegen_optional_field(&mut base.any_of, overlay.any_of);
        merge_non_codegen_optional_field(&mut base.one_of, overlay.one_of);

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

        match (
            base.additional_properties.take(),
            overlay.additional_properties,
        ) {
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
            (Some(left), Some(_right)) => {
                // Keep the left side; incompatible additionalProperties in allOf
                // is an under-specified schema — prefer the more descriptive branch.
                base.additional_properties = Some(left);
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
                        DiagnosticKind::IncompatibleAllOfField { field: key.clone() },
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

    fn collect_object_schema_view(
        &mut self,
        schema: &Schema,
        context: &str,
    ) -> Result<ObjectSchemaView> {
        let mut view = ObjectSchemaView::default();
        self.collect_object_schema_view_into(schema, context, &mut view)?;
        Ok(view)
    }

    fn collect_object_schema_view_into(
        &mut self,
        schema: &Schema,
        context: &str,
        view: &mut ObjectSchemaView,
    ) -> Result<()> {
        self.validate_schema_keywords(schema, context)?;

        if let Some(reference) = &schema.reference {
            if self
                .active_object_view_refs
                .iter()
                .any(|item| item == reference)
            {
                self.handle_unhandled(
                    context,
                    DiagnosticKind::AllOfRecursiveCycle {
                        reference: reference.clone(),
                    },
                )?;
                return Ok(());
            }

            self.active_object_view_refs.push(reference.clone());
            let resolved = self.resolve_schema_reference(reference)?;
            self.collect_object_schema_view_into(&resolved, reference, view)?;
            self.active_object_view_refs.pop();
        }

        if let Some(members) = &schema.all_of {
            for member in members {
                self.collect_object_schema_view_into(member, context, view)?;
            }
        }

        merge_non_codegen_optional_field(&mut view.title, schema.title.clone());

        if let Some(required) = &schema.required {
            view.required.extend(required.iter().cloned());
        }

        if let Some(properties) = &schema.properties {
            for (field_name, property_schema_or_bool) in properties {
                // Skip boolean schemas — they have no fields to contribute.
                let Some(property_schema) = property_schema_or_bool.as_schema() else {
                    continue;
                };
                if let Some(existing) = view.properties.shift_remove(field_name) {
                    view.properties.insert(
                        field_name.clone(),
                        self.merge_schemas(existing, property_schema.clone(), context)?,
                    );
                } else {
                    view.properties
                        .insert(field_name.clone(), property_schema.clone());
                }
            }
        }

        Ok(())
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
                    match schema.items.as_ref() {
                        Some(item_schema) => {
                            let imported = self.import_schema_type(item_schema, context)?;
                            ImportedType::plain(TypeRef::array(
                                imported
                                    .type_ref
                                    .unwrap_or_else(|| TypeRef::primitive("any")),
                            ))
                        }
                        // JSON Schema: array without `items` means array of any.
                        None => ImportedType::plain(TypeRef::array(TypeRef::primitive("any"))),
                    }
                }
                "object" => self.import_object_type(schema, context, None)?,
                other => {
                    self.handle_unhandled(
                        &context.describe(),
                        DiagnosticKind::UnsupportedSchemaType {
                            schema_type: other.to_owned(),
                        },
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
            Value::Object(_) => self.import_object_type(schema, context, None)?,
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
        local_reference: Option<&str>,
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
            let model_name = if let Some(reference) = local_reference {
                self.local_ref_model_names
                    .get(reference)
                    .cloned()
                    .unwrap_or_else(|| {
                        let model_name = self.inline_model_name(schema, context);
                        self.local_ref_model_names
                            .insert(reference.to_owned(), model_name.clone());
                        model_name
                    })
            } else {
                self.inline_model_name(schema, context)
            };

            if self.models.contains_key(&model_name)
                || self.active_model_builds.contains(&model_name)
            {
                return Ok(ImportedType::plain(TypeRef::named(model_name)));
            }

            self.active_model_builds.insert(model_name.clone());
            if !self.models.contains_key(&model_name) {
                let pointer = context.synthetic_pointer(&model_name);
                let build_result = self.build_model_from_schema(&model_name, schema, &pointer);
                self.active_model_builds.remove(&model_name);
                let model = build_result?;
                self.generated_model_names.insert(model_name.clone());
                self.models.insert(model_name.clone(), model);
            } else {
                self.active_model_builds.remove(&model_name);
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

    pub(crate) fn handle_unhandled(&mut self, context: &str, kind: DiagnosticKind) -> Result<()> {
        let diagnostic = self.make_diagnostic(context, kind);
        if self.options.ignore_unhandled {
            self.warnings.push(diagnostic);
            Ok(())
        } else {
            Err(anyhow::Error::new(diagnostic))
        }
    }

    /// Build an [`OpenApiDiagnostic`] from a context string (either a JSON
    /// pointer starting with `#/` or a human-readable label like
    /// `"parameter \`foo\`"`).
    fn make_diagnostic(&self, context: &str, kind: DiagnosticKind) -> OpenApiDiagnostic {
        if context.starts_with("#/") {
            let (preview, line) = self.source.pointer_info(context);
            OpenApiDiagnostic::from_pointer(kind, context, preview, line)
        } else {
            OpenApiDiagnostic::from_named_context(kind, context)
        }
    }

    /// Build a pointer diagnostic using the importer's source for preview
    /// rendering.
    fn make_pointer_diagnostic(&self, pointer: &str, kind: DiagnosticKind) -> OpenApiDiagnostic {
        let (preview, line) = self.source.pointer_info(pointer);
        OpenApiDiagnostic::from_pointer(kind, pointer, preview, line)
    }
}


#[derive(Debug, Clone)]
pub(crate) struct ImportedType {
    pub(crate) type_ref: Option<TypeRef>,
    pub(crate) nullable: bool,
}

impl ImportedType {
    fn plain(type_ref: TypeRef) -> Self {
        Self {
            type_ref: Some(type_ref),
            nullable: false,
        }
    }
}

#[derive(Default)]
pub(crate) struct ObjectSchemaView {
    pub(crate) title: Option<String>,
    pub(crate) properties: IndexMap<String, Schema>,
    pub(crate) required: BTreeSet<String>,
}

#[derive(Debug)]
pub(crate) enum InlineModelContext {
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
            InlineModelContext::Field { pointer, .. } => pointer.clone(),
            InlineModelContext::RequestBody { pointer, .. } => pointer.clone(),
            InlineModelContext::Response { pointer, .. } => pointer.clone(),
            InlineModelContext::Parameter { name } => format!("parameter `{name}`"),
        }
    }

    fn synthetic_pointer(&self, model_name: &str) -> String {
        match self {
            Self::NamedSchema { pointer, .. } => pointer.clone(),
            Self::Field { pointer, .. } => pointer.clone(),
            Self::RequestBody { pointer, .. } => pointer.clone(),
            Self::Response { pointer, .. } => pointer.clone(),
            Self::Parameter { name } => format!("#/synthetic/parameters/{name}/{model_name}"),
        }
    }
}
