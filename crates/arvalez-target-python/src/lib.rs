use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use arvalez_ir::{
    CoreIr, HttpMethod, Operation, ParameterLocation, RequestBody, Response, TypeRef,
};
use serde::Serialize;
use serde_json::{Value, from_value};
use tera::{Context as TeraContext, Tera};

const TEMPLATE_PYPROJECT: &str = "package/pyproject.toml.tera";
const TEMPLATE_README: &str = "package/README.md.tera";
const TEMPLATE_INIT: &str = "package/__init__.py.tera";
const TEMPLATE_MODELS: &str = "package/models.py.tera";
const TEMPLATE_CLIENT: &str = "package/client.py.tera";
const TEMPLATE_MODEL_CLASS: &str = "partials/model_class.py.tera";
const TEMPLATE_CLIENT_CLASS: &str = "partials/client_class.py.tera";
const TEMPLATE_TAG_CLIENT_CLASS: &str = "partials/tag_client_class.py.tera";
const TEMPLATE_CLIENT_METHOD: &str = "partials/client_method.py.tera";

const BUILTIN_TEMPLATES: &[(&str, &str)] = &[
    (
        TEMPLATE_PYPROJECT,
        include_str!("../templates/package/pyproject.toml.tera"),
    ),
    (
        TEMPLATE_README,
        include_str!("../templates/package/README.md.tera"),
    ),
    (
        TEMPLATE_INIT,
        include_str!("../templates/package/__init__.py.tera"),
    ),
    (
        TEMPLATE_MODELS,
        include_str!("../templates/package/models.py.tera"),
    ),
    (
        TEMPLATE_CLIENT,
        include_str!("../templates/package/client.py.tera"),
    ),
    (
        TEMPLATE_MODEL_CLASS,
        include_str!("../templates/partials/model_class.py.tera"),
    ),
    (
        TEMPLATE_CLIENT_CLASS,
        include_str!("../templates/partials/client_class.py.tera"),
    ),
    (
        TEMPLATE_TAG_CLIENT_CLASS,
        include_str!("../templates/partials/tag_client_class.py.tera"),
    ),
    (
        TEMPLATE_CLIENT_METHOD,
        include_str!("../templates/partials/client_method.py.tera"),
    ),
];

const OVERRIDABLE_TEMPLATES: &[&str] = &[
    TEMPLATE_PYPROJECT,
    TEMPLATE_README,
    TEMPLATE_INIT,
    TEMPLATE_MODELS,
    TEMPLATE_CLIENT,
    TEMPLATE_MODEL_CLASS,
    TEMPLATE_CLIENT_CLASS,
    TEMPLATE_TAG_CLIENT_CLASS,
    TEMPLATE_CLIENT_METHOD,
];

#[derive(Debug, Clone)]
pub struct PythonPackageConfig {
    pub package_name: String,
    pub project_name: String,
    pub version: String,
    pub template_dir: Option<PathBuf>,
    pub group_by_tag: bool,
}

impl PythonPackageConfig {
    pub fn new(package_name: impl Into<String>) -> Self {
        let package_name = package_name.into();
        let project_name = package_name.replace('_', "-");
        Self {
            package_name,
            project_name,
            version: "0.1.0".into(),
            template_dir: None,
            group_by_tag: false,
        }
    }

    pub fn with_version(mut self, version: impl Into<String>) -> Self {
        self.version = version.into();
        self
    }

    pub fn with_template_dir(mut self, template_dir: Option<PathBuf>) -> Self {
        self.template_dir = template_dir;
        self
    }

    pub fn with_group_by_tag(mut self, group_by_tag: bool) -> Self {
        self.group_by_tag = group_by_tag;
        self
    }
}

#[derive(Debug, Clone)]
pub struct GeneratedFile {
    pub path: PathBuf,
    pub contents: String,
}

pub fn generate_package(ir: &CoreIr, config: &PythonPackageConfig) -> Result<Vec<GeneratedFile>> {
    let package_dir = PathBuf::from("src").join(&config.package_name);
    let tera = load_templates(config.template_dir.as_deref())?;
    let package_context = PackageTemplateContext::from_ir(ir, config, &tera)?;
    let mut template_context = TeraContext::new();
    template_context.insert("package", &package_context);

    Ok(vec![
        GeneratedFile {
            path: PathBuf::from("pyproject.toml"),
            contents: tera
                .render(TEMPLATE_PYPROJECT, &template_context)
                .context("failed to render pyproject template")?,
        },
        GeneratedFile {
            path: PathBuf::from("README.md"),
            contents: tera
                .render(TEMPLATE_README, &template_context)
                .context("failed to render README template")?,
        },
        GeneratedFile {
            path: package_dir.join("__init__.py"),
            contents: tera
                .render(TEMPLATE_INIT, &template_context)
                .context("failed to render package __init__ template")?,
        },
        GeneratedFile {
            path: package_dir.join("models.py"),
            contents: tera
                .render(TEMPLATE_MODELS, &template_context)
                .context("failed to render models template")?,
        },
        GeneratedFile {
            path: package_dir.join("client.py"),
            contents: tera
                .render(TEMPLATE_CLIENT, &template_context)
                .context("failed to render client template")?,
        },
        GeneratedFile {
            path: package_dir.join("py.typed"),
            contents: String::new(),
        },
    ])
}

pub fn write_package(output_dir: impl AsRef<Path>, files: &[GeneratedFile]) -> Result<()> {
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

fn load_templates(template_dir: Option<&Path>) -> Result<Tera> {
    let mut tera = Tera::default();
    for (name, contents) in BUILTIN_TEMPLATES {
        tera.add_raw_template(name, contents)
            .with_context(|| format!("failed to register builtin template `{name}`"))?;
    }

    if let Some(template_dir) = template_dir {
        for name in OVERRIDABLE_TEMPLATES {
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

#[derive(Debug, Serialize)]
struct PackageTemplateContext {
    package_name: String,
    project_name: String,
    version: String,
    model_names: Vec<String>,
    model_imports_block: String,
    model_exports_block: String,
    model_blocks: Vec<String>,
    client_names: Vec<String>,
    client_blocks: Vec<String>,
}

impl PackageTemplateContext {
    fn from_ir(ir: &CoreIr, config: &PythonPackageConfig, tera: &Tera) -> Result<Self> {
        let model_names = sorted_models(ir)
            .into_iter()
            .map(|model| sanitize_class_name(&model.name))
            .collect::<Vec<_>>();
        let model_imports_block = indent_block(
            &model_names
                .iter()
                .map(|name| format!("{name},"))
                .collect::<Vec<_>>(),
            4,
        );
        let model_exports_block = indent_block(
            &model_names
                .iter()
                .map(|name| format!("{name:?},"))
                .collect::<Vec<_>>(),
            4,
        );

        let model_blocks = sorted_models(ir)
            .into_iter()
            .map(|model| render_model_block(tera, ModelView::from_model(model)))
            .collect::<Result<Vec<_>>>()?;

        let client_layout = ClientLayout::from_ir(ir);
        let mut client_blocks = Vec::new();

        if config.group_by_tag {
            for tag_group in &client_layout.tag_groups {
                client_blocks.push(render_tag_client_block(
                    tera,
                    TagClientClassView::async_client(tag_group, tera)?,
                )?);
                client_blocks.push(render_tag_client_block(
                    tera,
                    TagClientClassView::sync_client(tag_group, tera)?,
                )?);
            }
        }

        let async_client = render_client_block(
            tera,
            ClientClassView::async_client(
                if config.group_by_tag {
                    &client_layout.untagged_operations
                } else {
                    &client_layout.all_operations
                },
                if config.group_by_tag {
                    &client_layout.tag_groups
                } else {
                    &[]
                },
                tera,
            )?,
        )?;
        let sync_client = render_client_block(
            tera,
            ClientClassView::sync_client(
                if config.group_by_tag {
                    &client_layout.untagged_operations
                } else {
                    &client_layout.all_operations
                },
                if config.group_by_tag {
                    &client_layout.tag_groups
                } else {
                    &[]
                },
                tera,
            )?,
        )?;
        client_blocks.push(async_client);
        client_blocks.push(sync_client);

        Ok(Self {
            package_name: config.package_name.clone(),
            project_name: config.project_name.clone(),
            version: config.version.clone(),
            model_names,
            model_imports_block,
            model_exports_block,
            model_blocks,
            client_names: vec![
                "ApiClient".into(),
                "AsyncApiClient".into(),
                "SyncApiClient".into(),
            ],
            client_blocks,
        })
    }
}

#[derive(Debug)]
struct ClientLayout<'a> {
    all_operations: Vec<&'a Operation>,
    untagged_operations: Vec<&'a Operation>,
    tag_groups: Vec<TagGroup<'a>>,
}

impl<'a> ClientLayout<'a> {
    fn from_ir(ir: &'a CoreIr) -> Self {
        let all_operations = sorted_operations(ir);
        let mut tag_map: BTreeMap<String, Vec<&Operation>> = BTreeMap::new();
        let mut untagged_operations = Vec::new();

        for operation in &all_operations {
            match operation_primary_tag(operation) {
                Some(tag) => tag_map.entry(tag).or_default().push(*operation),
                None => untagged_operations.push(*operation),
            }
        }

        let tag_groups = tag_map
            .into_iter()
            .map(|(tag, operations)| TagGroup::new(tag, operations))
            .collect::<Vec<_>>();

        Self {
            all_operations,
            untagged_operations,
            tag_groups,
        }
    }
}

#[derive(Debug)]
struct TagGroup<'a> {
    property_name: String,
    class_base_name: String,
    operations: Vec<&'a Operation>,
}

impl<'a> TagGroup<'a> {
    fn new(tag: String, operations: Vec<&'a Operation>) -> Self {
        Self {
            property_name: sanitize_identifier(&tag),
            class_base_name: sanitize_class_name(&tag),
            operations,
        }
    }
}

#[derive(Debug, Serialize)]
struct ModelView {
    class_name: String,
    has_fields: bool,
    is_enum: bool,
    is_alias: bool,
    enum_base_classes: String,
    enum_members_block: String,
    alias_expression: String,
    fields_block: String,
}

impl ModelView {
    fn from_model(model: &arvalez_ir::Model) -> Self {
        let field_lines = model
            .fields
            .iter()
            .map(ModelFieldView::from_field)
            .map(|field| field.declaration)
            .collect::<Vec<_>>();
        let enum_members = model
            .attributes
            .get("enum_values")
            .and_then(|value| value.as_array())
            .map(|values| {
                values
                    .iter()
                    .enumerate()
                    .map(|(index, value)| render_enum_member(value, index))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let is_enum = !enum_members.is_empty();
        let alias_type_ref = model
            .attributes
            .get("alias_type_ref")
            .cloned()
            .and_then(|value| from_value::<TypeRef>(value).ok());
        let alias_nullable = model
            .attributes
            .get("alias_nullable")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        let is_alias = alias_type_ref.is_some();

        Self {
            class_name: sanitize_class_name(&model.name),
            is_enum,
            is_alias,
            enum_base_classes: enum_base_classes(model),
            enum_members_block: indent_block(&enum_members, 4),
            alias_expression: alias_type_ref
                .map(|type_ref| python_field_type(&type_ref, false, alias_nullable))
                .unwrap_or_default(),
            has_fields: !field_lines.is_empty(),
            fields_block: indent_block(&field_lines, 4),
        }
    }
}

#[derive(Debug)]
struct ModelFieldView {
    declaration: String,
}

impl ModelFieldView {
    fn from_field(field: &arvalez_ir::Field) -> Self {
        let python_name = sanitize_identifier(&field.name);
        let type_hint = python_field_type(&field.type_ref, field.optional, field.nullable);
        let default_value = field.optional.then_some("None");
        let declaration = if python_name == field.name {
            match default_value {
                Some(default_value) => format!("{python_name}: {type_hint} = {default_value}"),
                None => format!("{python_name}: {type_hint}"),
            }
        } else {
            match default_value {
                Some(default_value) => format!(
                    "{python_name}: {type_hint} = Field(default={default_value}, alias={:?})",
                    field.name
                ),
                None => format!("{python_name}: {type_hint} = Field(alias={:?})", field.name),
            }
        };

        Self { declaration }
    }
}

#[derive(Debug, Serialize)]
struct ClientClassView {
    class_name: String,
    client_type: String,
    service_bindings_block: String,
    close_method_signature: String,
    close_method_block: String,
    enter_method_signature: String,
    exit_method_signature: String,
    exit_method_block: String,
    methods_block: String,
}

impl ClientClassView {
    fn async_client(
        operations: &[&Operation],
        tag_groups: &[TagGroup<'_>],
        tera: &Tera,
    ) -> Result<Self> {
        let methods = operations
            .into_iter()
            .map(|operation| OperationMethodView::from_operation(operation, ClientMode::Async))
            .collect::<Vec<_>>();
        Ok(Self {
            class_name: "AsyncApiClient".into(),
            client_type: "httpx.AsyncClient".into(),
            service_bindings_block: indent_block(
                &tag_groups
                    .iter()
                    .map(|group| {
                        format!(
                            "self.{} = Async{}Api(self)",
                            group.property_name, group.class_base_name
                        )
                    })
                    .collect::<Vec<_>>(),
                8,
            ),
            close_method_signature: "async def aclose(self) -> None:".into(),
            close_method_block: indent_block(
                &[
                    "if self._owns_client:".into(),
                    "    await self._client.aclose()".into(),
                ],
                8,
            ),
            enter_method_signature: "async def __aenter__(self) -> AsyncApiClient:".into(),
            exit_method_signature:
                "async def __aexit__(self, exc_type: Any, exc: Any, tb: Any) -> None:".into(),
            exit_method_block: indent_block(&["await self.aclose()".into()], 8),
            methods_block: render_methods_block(tera, methods)?,
        })
    }

    fn sync_client(
        operations: &[&Operation],
        tag_groups: &[TagGroup<'_>],
        tera: &Tera,
    ) -> Result<Self> {
        let methods = operations
            .into_iter()
            .map(|operation| OperationMethodView::from_operation(operation, ClientMode::Sync))
            .collect::<Vec<_>>();
        Ok(Self {
            class_name: "SyncApiClient".into(),
            client_type: "httpx.Client".into(),
            service_bindings_block: indent_block(
                &tag_groups
                    .iter()
                    .map(|group| {
                        format!(
                            "self.{} = Sync{}Api(self)",
                            group.property_name, group.class_base_name
                        )
                    })
                    .collect::<Vec<_>>(),
                8,
            ),
            close_method_signature: "def close(self) -> None:".into(),
            close_method_block: indent_block(
                &[
                    "if self._owns_client:".into(),
                    "    self._client.close()".into(),
                ],
                8,
            ),
            enter_method_signature: "def __enter__(self) -> SyncApiClient:".into(),
            exit_method_signature: "def __exit__(self, exc_type: Any, exc: Any, tb: Any) -> None:"
                .into(),
            exit_method_block: indent_block(&["self.close()".into()], 8),
            methods_block: render_methods_block(tera, methods)?,
        })
    }
}

#[derive(Debug, Serialize)]
struct TagClientClassView {
    class_name: String,
    owner_class_name: String,
    methods_block: String,
}

impl TagClientClassView {
    fn async_client(tag_group: &TagGroup<'_>, tera: &Tera) -> Result<Self> {
        let methods = tag_group
            .operations
            .iter()
            .map(|operation| OperationMethodView::from_operation(operation, ClientMode::Async))
            .collect::<Vec<_>>();

        Ok(Self {
            class_name: format!("Async{}Api", tag_group.class_base_name),
            owner_class_name: "AsyncApiClient".into(),
            methods_block: render_methods_block(tera, methods)?,
        })
    }

    fn sync_client(tag_group: &TagGroup<'_>, tera: &Tera) -> Result<Self> {
        let methods = tag_group
            .operations
            .iter()
            .map(|operation| OperationMethodView::from_operation(operation, ClientMode::Sync))
            .collect::<Vec<_>>();

        Ok(Self {
            class_name: format!("Sync{}Api", tag_group.class_base_name),
            owner_class_name: "SyncApiClient".into(),
            methods_block: render_methods_block(tera, methods)?,
        })
    }
}

#[derive(Debug, Clone, Copy)]
enum ClientMode {
    Async,
    Sync,
}

#[derive(Debug, Serialize)]
struct OperationMethodView {
    def_keyword: String,
    method_name: String,
    raw_method_name: String,
    args_signature: String,
    return_annotation: String,
    raw_return_annotation: String,
    url_template: String,
    raw_request_call_line: String,
    raw_pre_request_block: String,
    raw_post_request_block: String,
    wrapper_request_call_line: String,
    post_request_block: String,
}

impl OperationMethodView {
    fn from_operation(operation: &Operation, mode: ClientMode) -> Self {
        let mut pre_request_lines = Vec::new();
        let mut call_arguments = Vec::new();
        let mut params_name = "None".to_owned();
        let mut headers_name = "None".to_owned();

        let query_params = operation
            .params
            .iter()
            .filter(|param| matches!(param.location, ParameterLocation::Query))
            .collect::<Vec<_>>();
        if !query_params.is_empty() {
            pre_request_lines.push("params: dict[str, Any] = {}".into());
            params_name = "params".into();
            for param in query_params {
                let param_name = sanitize_identifier(&param.name);
                if param.required {
                    pre_request_lines.push(format!("params[{:?}] = {param_name}", param.name));
                } else {
                    pre_request_lines.push(format!("if {param_name} is not None:"));
                    pre_request_lines.push(format!("    params[{:?}] = {param_name}", param.name));
                }
            }
            call_arguments.push("params=params".into());
        }

        let header_params = operation
            .params
            .iter()
            .filter(|param| matches!(param.location, ParameterLocation::Header))
            .collect::<Vec<_>>();
        if !header_params.is_empty() {
            pre_request_lines.push("headers: dict[str, Any] = {}".into());
            headers_name = "headers".into();
            for param in header_params {
                let param_name = sanitize_identifier(&param.name);
                if param.required {
                    pre_request_lines.push(format!("headers[{:?}] = {param_name}", param.name));
                } else {
                    pre_request_lines.push(format!("if {param_name} is not None:"));
                    pre_request_lines.push(format!("    headers[{:?}] = {param_name}", param.name));
                }
            }
            call_arguments.push("headers=headers".into());
        }

        let uses_request_kwargs = true;
        pre_request_lines.push("request_kwargs: dict[str, Any] = {}".into());
        if let Some(request_body) = &operation.request_body {
            if request_body.required {
                call_arguments.push(required_request_body_argument(request_body));
            } else {
                pre_request_lines.extend(optional_request_body_lines(request_body));
            }
        }
        pre_request_lines.push(format!(
            "request_kwargs = self._apply_request_options(request_kwargs, request_options, params={params_name}, headers={headers_name})"
        ));

        let return_type = operation_return_type(operation);
        let mut post_request_lines = vec!["self._handle_error(response, request_options)".into()];
        if let Some(parse_expression) = return_type.parse_expression.clone() {
            post_request_lines.push(format!(
                "return self._parse_response(response, {parse_expression})"
            ));
        } else {
            post_request_lines.push("return self._parse_response(response)".into());
        }

        let request_suffix = render_request_arguments(&call_arguments, uses_request_kwargs);
        let raw_request_call_line = match mode {
            ClientMode::Async => format!(
                "response = await self._client.request({:?}, url{request_suffix})",
                method_literal(operation.method)
            ),
            ClientMode::Sync => format!(
                "response = self._client.request({:?}, url{request_suffix})",
                method_literal(operation.method)
            ),
        };
        let wrapper_request_call_line = match mode {
            ClientMode::Async => format!(
                "response = await self.{}({})",
                raw_method_name(operation),
                build_wrapper_forward_arguments(operation)
            ),
            ClientMode::Sync => format!(
                "response = self.{}({})",
                raw_method_name(operation),
                build_wrapper_forward_arguments(operation)
            ),
        };

        Self {
            def_keyword: match mode {
                ClientMode::Async => "async def".into(),
                ClientMode::Sync => "def".into(),
            },
            method_name: sanitize_identifier(&operation.name),
            raw_method_name: raw_method_name(operation),
            args_signature: build_method_args(operation).join(", "),
            return_annotation: return_type.annotation.unwrap_or_else(|| "None".into()),
            raw_return_annotation: "httpx.Response".into(),
            url_template: render_python_path_template(&operation.path),
            raw_request_call_line,
            raw_pre_request_block: indent_block(&pre_request_lines, 8),
            raw_post_request_block: indent_block(&["return response".into()], 8),
            wrapper_request_call_line,
            post_request_block: indent_block(&post_request_lines, 8),
        }
    }
}

fn render_request_arguments(call_arguments: &[String], uses_request_kwargs: bool) -> String {
    let mut arguments = call_arguments.to_vec();
    if uses_request_kwargs {
        arguments.push("**request_kwargs".into());
    }
    if arguments.is_empty() {
        String::new()
    } else {
        format!(", {}", arguments.join(", "))
    }
}

fn required_request_body_argument(request_body: &RequestBody) -> String {
    let (body_kwarg, json_mode_literal) = request_body_binding(request_body);
    format!("{body_kwarg}=self._serialize_body(body, json_mode={json_mode_literal})")
}

fn optional_request_body_lines(request_body: &RequestBody) -> Vec<String> {
    let (body_kwarg, json_mode_literal) = request_body_binding(request_body);
    vec![
        "if body is not None:".into(),
        format!(
            "    request_kwargs[{body_kwarg:?}] = self._serialize_body(body, json_mode={json_mode_literal})"
        ),
    ]
}

fn request_body_binding(request_body: &RequestBody) -> (&'static str, &'static str) {
    let json_mode = request_body.media_type == "application/json";
    let json_mode_literal = if json_mode { "True" } else { "False" };
    let body_kwarg = if json_mode { "json" } else { "data" };
    (body_kwarg, json_mode_literal)
}

fn render_model_block(tera: &Tera, model: ModelView) -> Result<String> {
    let mut context = TeraContext::new();
    context.insert("model", &model);
    tera.render(TEMPLATE_MODEL_CLASS, &context)
        .context("failed to render model class partial")
}

fn render_client_block(tera: &Tera, client: ClientClassView) -> Result<String> {
    let mut context = TeraContext::new();
    context.insert("client", &client);
    tera.render(TEMPLATE_CLIENT_CLASS, &context)
        .context("failed to render client class partial")
}

fn render_tag_client_block(tera: &Tera, client: TagClientClassView) -> Result<String> {
    let mut context = TeraContext::new();
    context.insert("client", &client);
    tera.render(TEMPLATE_TAG_CLIENT_CLASS, &context)
        .context("failed to render tag client class partial")
}

fn render_methods_block(tera: &Tera, methods: Vec<OperationMethodView>) -> Result<String> {
    methods
        .into_iter()
        .map(|method| render_method_block(tera, method))
        .collect::<Result<Vec<_>>>()
        .map(|methods| methods.join("\n"))
}

fn render_method_block(tera: &Tera, method: OperationMethodView) -> Result<String> {
    let mut context = TeraContext::new();
    context.insert("operation", &method);
    tera.render(TEMPLATE_CLIENT_METHOD, &context)
        .context("failed to render client method partial")
}

fn raw_method_name(operation: &Operation) -> String {
    format!("_{}_raw", sanitize_identifier(&operation.name))
}

fn build_wrapper_forward_arguments(operation: &Operation) -> String {
    let mut arguments = Vec::new();

    for param in operation.params.iter().filter(|param| param.required) {
        let name = sanitize_identifier(&param.name);
        arguments.push(format!("{name}={name}"));
    }

    if let Some(request_body) = &operation.request_body
        && request_body.required
    {
        let _ = request_body;
        arguments.push("body=body".into());
    }

    for param in operation.params.iter().filter(|param| !param.required) {
        let name = sanitize_identifier(&param.name);
        arguments.push(format!("{name}={name}"));
    }

    if let Some(request_body) = &operation.request_body
        && !request_body.required
    {
        let _ = request_body;
        arguments.push("body=body".into());
    }

    arguments.push("request_options=request_options".into());
    arguments.join(", ")
}

fn indent_block(lines: &[String], spaces: usize) -> String {
    let indent = " ".repeat(spaces);
    lines
        .iter()
        .map(|line| format!("{indent}{line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn enum_base_classes(model: &arvalez_ir::Model) -> String {
    match model
        .attributes
        .get("enum_base_type")
        .and_then(|value| value.as_str())
    {
        Some("string") => "str, Enum".into(),
        Some("integer") => "int, Enum".into(),
        Some("number") => "float, Enum".into(),
        _ => "Enum".into(),
    }
}

fn render_enum_member(value: &Value, index: usize) -> String {
    let member_name = value
        .as_str()
        .map(sanitize_enum_member_name)
        .unwrap_or_else(|| format!("VALUE_{index}"));
    format!("{member_name} = {}", python_literal(value))
}

fn sanitize_enum_member_name(value: &str) -> String {
    let mut candidate = String::new();
    let mut last_was_separator = false;
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            candidate.push(ch.to_ascii_uppercase());
            last_was_separator = false;
        } else if !last_was_separator && !candidate.is_empty() {
            candidate.push('_');
            last_was_separator = true;
        }
    }
    while candidate.ends_with('_') {
        candidate.pop();
    }
    if candidate.is_empty() {
        candidate = "VALUE".into();
    }
    if candidate
        .chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_digit())
    {
        candidate.insert(0, '_');
    }
    if is_python_keyword(&candidate.to_ascii_lowercase()) {
        candidate.push('_');
    }
    candidate
}

fn python_literal(value: &Value) -> String {
    match value {
        Value::String(value) => format!("{value:?}"),
        Value::Number(value) => value.to_string(),
        Value::Bool(value) => {
            if *value {
                "True".into()
            } else {
                "False".into()
            }
        }
        Value::Null => "None".into(),
        _ => serde_json::to_string(value).unwrap_or_else(|_| "None".into()),
    }
}

#[derive(Debug, Clone)]
struct ReturnType {
    annotation: Option<String>,
    parse_expression: Option<String>,
}

fn operation_return_type(operation: &Operation) -> ReturnType {
    let success = operation
        .responses
        .iter()
        .find(|response| response.status.starts_with('2'));
    success.map(response_return_type).unwrap_or(ReturnType {
        annotation: None,
        parse_expression: None,
    })
}

fn response_return_type(response: &Response) -> ReturnType {
    match &response.type_ref {
        Some(type_ref) => {
            let type_hint = python_type_ref(type_ref, PythonContext::Client);
            ReturnType {
                annotation: Some(type_hint.clone()),
                parse_expression: Some(type_hint),
            }
        }
        None => ReturnType {
            annotation: Some("None".into()),
            parse_expression: None,
        },
    }
}

fn build_method_args(operation: &Operation) -> Vec<String> {
    let mut args = Vec::new();

    for param in operation.params.iter().filter(|param| param.required) {
        args.push(format!(
            "{}: {}",
            sanitize_identifier(&param.name),
            python_type_ref(&param.type_ref, PythonContext::Client)
        ));
    }

    if let Some(request_body) = &operation.request_body
        && request_body.required
    {
        args.push(format!(
            "body: {}",
            request_body
                .type_ref
                .as_ref()
                .map(|type_ref| python_type_ref(type_ref, PythonContext::Client))
                .unwrap_or_else(|| "Any".into())
        ));
    }

    for param in operation.params.iter().filter(|param| !param.required) {
        args.push(format!(
            "{}: {} | None = None",
            sanitize_identifier(&param.name),
            python_type_ref(&param.type_ref, PythonContext::Client)
        ));
    }

    if let Some(request_body) = &operation.request_body
        && !request_body.required
    {
        args.push(format!(
            "body: {} | None = None",
            request_body
                .type_ref
                .as_ref()
                .map(|type_ref| python_type_ref(type_ref, PythonContext::Client))
                .unwrap_or_else(|| "Any".into())
        ));
    }

    args.push("request_options: RequestOptions | None = None".into());

    args
}

fn sorted_models(ir: &CoreIr) -> Vec<&arvalez_ir::Model> {
    let mut models = ir.models.iter().collect::<Vec<_>>();
    models.sort_by(|left, right| left.name.cmp(&right.name));
    models
}

fn sorted_operations(ir: &CoreIr) -> Vec<&Operation> {
    let mut operations = ir.operations.iter().collect::<Vec<_>>();
    operations.sort_by(|left, right| left.name.cmp(&right.name));
    operations
}

fn operation_primary_tag(operation: &Operation) -> Option<String> {
    operation
        .attributes
        .get("tags")
        .and_then(|value| value.as_array())
        .and_then(|tags| tags.first())
        .and_then(|tag| tag.as_str())
        .map(str::trim)
        .filter(|tag| !tag.is_empty())
        .map(ToOwned::to_owned)
}

#[derive(Debug, Clone, Copy)]
enum PythonContext {
    Models,
    Client,
}

fn python_field_type(type_ref: &TypeRef, optional: bool, nullable: bool) -> String {
    let mut type_hint = python_type_ref(type_ref, PythonContext::Models);
    if optional || nullable {
        type_hint.push_str(" | None");
    }
    type_hint
}

fn python_type_ref(type_ref: &TypeRef, context: PythonContext) -> String {
    match type_ref {
        TypeRef::Primitive { name } => match name.as_str() {
            "string" => "str".into(),
            "integer" => "int".into(),
            "number" => "float".into(),
            "boolean" => "bool".into(),
            "binary" => "bytes".into(),
            "null" => "None".into(),
            "any" | "object" => "Any".into(),
            _ => "Any".into(),
        },
        TypeRef::Named { name } => match context {
            PythonContext::Models => sanitize_class_name(name),
            PythonContext::Client => format!("models.{}", sanitize_class_name(name)),
        },
        TypeRef::Array { item } => format!("list[{}]", python_type_ref(item, context)),
        TypeRef::Map { value } => format!("dict[str, {}]", python_type_ref(value, context)),
        TypeRef::Union { variants } => variants
            .iter()
            .map(|variant| python_type_ref(variant, context))
            .collect::<Vec<_>>()
            .join(" | "),
    }
}

fn method_literal(method: HttpMethod) -> &'static str {
    match method {
        HttpMethod::Get => "GET",
        HttpMethod::Post => "POST",
        HttpMethod::Put => "PUT",
        HttpMethod::Patch => "PATCH",
        HttpMethod::Delete => "DELETE",
    }
}

fn render_python_path_template(path: &str) -> String {
    let mut result = String::from("f\"");
    let mut chars = path.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '{' => {
                let mut name = String::new();
                while let Some(next) = chars.peek() {
                    if *next == '}' {
                        chars.next();
                        break;
                    }
                    name.push(*next);
                    chars.next();
                }
                result.push('{');
                result.push_str(&sanitize_identifier(&name));
                result.push('}');
            }
            '"' => result.push_str("\\\""),
            _ => result.push(ch),
        }
    }
    result.push('"');
    result
}

fn sanitize_class_name(name: &str) -> String {
    let mut out = String::new();
    for part in split_words(name) {
        let mut chars = part.chars();
        if let Some(first) = chars.next() {
            out.extend(first.to_uppercase());
            out.push_str(chars.as_str());
        }
    }
    if out.is_empty() {
        "GeneratedModel".into()
    } else if out.chars().next().is_some_and(|ch| ch.is_ascii_digit()) {
        format!("_{out}")
    } else {
        out
    }
}

fn sanitize_identifier(name: &str) -> String {
    let words = split_words(name);
    let mut candidate = if words.is_empty() {
        "value".into()
    } else {
        words.join("_")
    };
    if candidate
        .chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_digit())
    {
        candidate.insert(0, '_');
    }
    if is_python_keyword(&candidate) {
        candidate.push('_');
    }
    candidate
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

fn is_python_keyword(value: &str) -> bool {
    matches!(
        value,
        "and"
            | "as"
            | "assert"
            | "async"
            | "await"
            | "break"
            | "class"
            | "continue"
            | "def"
            | "del"
            | "elif"
            | "else"
            | "except"
            | "false"
            | "finally"
            | "for"
            | "from"
            | "global"
            | "if"
            | "import"
            | "in"
            | "is"
            | "lambda"
            | "none"
            | "nonlocal"
            | "not"
            | "or"
            | "pass"
            | "raise"
            | "return"
            | "true"
            | "try"
            | "while"
            | "with"
            | "yield"
    )
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;
    use tempfile::tempdir;

    use super::*;
    use arvalez_ir::{Attributes, Field, Parameter};

    fn sample_ir() -> CoreIr {
        CoreIr {
            models: vec![
                arvalez_ir::Model {
                    id: "model.widget_path".into(),
                    name: "WidgetPath".into(),
                    fields: Vec::new(),
                    attributes: Attributes::from([
                        ("alias_type_ref".into(), json!(TypeRef::primitive("string"))),
                        ("alias_nullable".into(), json!(false)),
                    ]),
                    source: None,
                },
                arvalez_ir::Model {
                    id: "model.widget_status".into(),
                    name: "WidgetStatus".into(),
                    fields: Vec::new(),
                    attributes: Attributes::from([
                        ("enum_base_type".into(), json!("string")),
                        ("enum_values".into(), json!(["READY", "PAUSED"])),
                    ]),
                    source: None,
                },
                arvalez_ir::Model {
                    id: "model.widget".into(),
                    name: "Widget".into(),
                    fields: vec![
                        Field::new("id", TypeRef::primitive("string")),
                        Field::new("path", TypeRef::named("WidgetPath")),
                        Field::new("status", TypeRef::named("WidgetStatus")),
                        Field {
                            name: "count".into(),
                            type_ref: TypeRef::primitive("integer"),
                            optional: true,
                            nullable: false,
                            attributes: Attributes::default(),
                        },
                    ],
                    attributes: Attributes::default(),
                    source: None,
                },
            ],
            operations: vec![
                Operation {
                    id: "operation.get_widget".into(),
                    name: "get_widget".into(),
                    method: HttpMethod::Get,
                    path: "/widgets/{widget_id}".into(),
                    params: vec![Parameter {
                        name: "widget_id".into(),
                        location: ParameterLocation::Path,
                        type_ref: TypeRef::primitive("string"),
                        required: true,
                    }],
                    request_body: Some(RequestBody {
                        required: false,
                        media_type: "application/json".into(),
                        type_ref: Some(TypeRef::named("Widget")),
                    }),
                    responses: vec![Response {
                        status: "200".into(),
                        media_type: Some("application/json".into()),
                        type_ref: Some(TypeRef::named("Widget")),
                        attributes: Attributes::default(),
                    }],
                    attributes: Attributes::from([("tags".into(), json!(["widgets"]))]),
                    source: None,
                },
                Operation {
                    id: "operation.healthcheck".into(),
                    name: "healthcheck".into(),
                    method: HttpMethod::Get,
                    path: "/healthcheck".into(),
                    params: Vec::new(),
                    request_body: None,
                    responses: vec![Response {
                        status: "204".into(),
                        media_type: None,
                        type_ref: None,
                        attributes: Attributes::default(),
                    }],
                    attributes: Attributes::default(),
                    source: None,
                },
            ],
            ..Default::default()
        }
    }

    #[test]
    fn renders_basic_python_package() {
        let files = generate_package(&sample_ir(), &PythonPackageConfig::new("demo_client"))
            .expect("package should render");
        let init = files
            .iter()
            .find(|file| file.path.ends_with("__init__.py"))
            .expect("__init__.py");
        let client = files
            .iter()
            .find(|file| file.path.ends_with("client.py"))
            .expect("client.py");
        let models = files
            .iter()
            .find(|file| file.path.ends_with("models.py"))
            .expect("models.py");

        assert!(init.contents.contains("AsyncApiClient"));
        assert!(init.contents.contains("ErrorHandler"));
        assert!(init.contents.contains("RequestOptions"));
        assert!(init.contents.contains("SyncApiClient"));
        assert!(models.contents.contains("from enum import Enum"));
        assert!(models.contents.contains("WidgetPath = str"));
        assert!(models.contents.contains("class WidgetStatus(str, Enum):"));
        assert!(
            models.contents.contains("READY = \"READY\"")
                || models.contents.contains("READY = 'READY'")
        );
        assert!(models.contents.contains("path: WidgetPath"));
        assert!(models.contents.contains("status: WidgetStatus"));
        assert!(
            client
                .contents
                .contains("class AsyncApiClient(_BaseApiClient):")
        );
        assert!(
            client
                .contents
                .contains("class SyncApiClient(_BaseApiClient):")
        );
        assert!(client.contents.contains("ApiClient = AsyncApiClient"));
        assert!(
            client
                .contents
                .contains("class RequestOptions(TypedDict, total=False):")
        );
        assert!(client.contents.contains("on_error: ErrorHandler"));
        assert!(
            client
                .contents
                .contains("on_error: ErrorHandler | None = None")
        );
        assert!(client.contents.contains("async def get_widget"));
        assert!(client.contents.contains("async def _get_widget_raw"));
        assert!(client.contents.contains("def get_widget"));
        assert!(client.contents.contains("def _get_widget_raw"));
        assert!(
            client
                .contents
                .contains("request_options: RequestOptions | None = None")
        );
        assert!(client.contents.contains("async def healthcheck"));
        assert!(client.contents.contains("def healthcheck"));
        assert!(
            client
                .contents
                .contains("request_kwargs = self._apply_request_options(request_kwargs, request_options, params=None, headers=None)")
        );
        assert!(
            client
                .contents
                .contains("self._handle_error(response, request_options)")
        );
        assert!(
            client
                .contents
                .contains("response = await self._client.request(\"GET\", url, **request_kwargs)")
        );
        assert!(
            client
                .contents
                .contains("response = await self._get_widget_raw(")
        );
        assert!(
            client
                .contents
                .contains("response = self._client.request(\"GET\", url, **request_kwargs)")
        );
        assert!(client.contents.contains("response = self._get_widget_raw("));
    }

    #[test]
    fn supports_selective_template_overrides() {
        let tempdir = tempdir().expect("tempdir");
        let partial_dir = tempdir.path().join("partials");
        fs::create_dir_all(&partial_dir).expect("partials dir");
        fs::write(
            partial_dir.join("client_class.py.tera"),
            "class {{ client.class_name }}:\n    OVERRIDDEN = True\n",
        )
        .expect("override template");

        let config = PythonPackageConfig::new("demo_client")
            .with_template_dir(Some(tempdir.path().to_path_buf()));
        let files = generate_package(&sample_ir(), &config).expect("package should render");
        let client = files
            .iter()
            .find(|file| file.path.ends_with("client.py"))
            .expect("client.py");

        assert!(
            client
                .contents
                .contains("class AsyncApiClient:\n    OVERRIDDEN = True")
        );
        assert!(
            client
                .contents
                .contains("class SyncApiClient:\n    OVERRIDDEN = True")
        );
    }

    #[test]
    fn groups_operations_by_tag_when_enabled() {
        let config = PythonPackageConfig::new("demo_client").with_group_by_tag(true);
        let files = generate_package(&sample_ir(), &config).expect("package should render");
        let client = files
            .iter()
            .find(|file| file.path.ends_with("client.py"))
            .expect("client.py");

        assert!(
            client
                .contents
                .contains("class AsyncWidgetsApi(_BaseApiClient):")
        );
        assert!(
            client
                .contents
                .contains("class SyncWidgetsApi(_BaseApiClient):")
        );
        assert!(
            client
                .contents
                .contains("self.widgets = AsyncWidgetsApi(self)")
        );
        assert!(
            client
                .contents
                .contains("self.widgets = SyncWidgetsApi(self)")
        );
        assert!(client.contents.contains("async def get_widget"));
        assert!(client.contents.contains("def get_widget"));
        assert!(client.contents.contains("async def healthcheck"));
        assert!(client.contents.contains("def healthcheck"));
    }
}
