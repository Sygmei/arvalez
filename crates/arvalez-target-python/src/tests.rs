use std::fs;

use serde_json::json;
use tempfile::tempdir;

use crate::sanitize::{sanitize_class_name, sanitize_identifier, sanitize_subsection_name};
use crate::{generate, CommonConfig, GeneratedFile, TargetConfig};
use arvalez_ir::{
    Attributes, CoreIr, Field, HttpMethod, Operation, Parameter, ParameterLocation, RequestBody,
    Response, TypeRef,
};
use serde_json::Value;

fn make_package(
    package_name: &str,
    template_dir: Option<std::path::PathBuf>,
    target: TargetConfig,
) -> anyhow::Result<Vec<GeneratedFile>> {
    make_package_from_ir(sample_ir(), package_name, template_dir, target)
}

fn make_package_from_ir(
    ir: CoreIr,
    package_name: &str,
    template_dir: Option<std::path::PathBuf>,
    target: TargetConfig,
) -> anyhow::Result<Vec<GeneratedFile>> {
    let common = CommonConfig {
        package: arvalez_target_core::PackageConfig {
            name: package_name.into(),
            version: "0.1.0".into(),
            description: None,
        },
    };
    generate(&ir, template_dir.as_deref(), &common, &target)
}

fn sample_ir() -> arvalez_ir::CoreIr {
    arvalez_ir::CoreIr {
        models: vec![
            arvalez_ir::Model {
                id: "model.widget_path".into(),
                name: "WidgetPath".into(),
                kind: arvalez_ir::ModelKind::Object,
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
                kind: arvalez_ir::ModelKind::Enum {
                    base: TypeRef::primitive("string"),
                    values: vec![json!("READY"), json!("PAUSED")],
                },
                fields: Vec::new(),
                attributes: Attributes::default(),
                source: None,
            },
            arvalez_ir::Model {
                id: "model.widget".into(),
                name: "Widget".into(),
                kind: arvalez_ir::ModelKind::Object,
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
                    attributes: Attributes::from([(
                        "description".into(),
                        Value::String("Unique widget identifier.".into()),
                    )]),
                }],
                request_body: Some(RequestBody {
                    required: false,
                    media_type: "application/json".into(),
                    type_ref: Some(TypeRef::named("Widget")),
                    attributes: Attributes::default(),
                }),
                responses: vec![Response {
                    status: "200".into(),
                    media_type: Some("application/json".into()),
                    type_ref: Some(TypeRef::named("Widget")),
                    attributes: Attributes::default(),
                }],
                attributes: Attributes::from([
                    ("tags".into(), json!(["widgets"])),
                    ("deprecated".into(), json!(true)),
                ]),
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
    let files =
        make_package("demo_client", None, TargetConfig::default()).expect("package should render");
    let init = files
        .iter()
        .find(|file| file.path.ends_with("__init__.py"))
        .expect("__init__.py");
    let client = files
        .iter()
        .find(|file| file.path.ends_with("client.py"))
        .expect("client.py");
    let utils = files
        .iter()
        .find(|file| file.path.ends_with("utils.py"))
        .expect("utils.py");
    let models = files
        .iter()
        .find(|file| file.path.ends_with("models.py"))
        .expect("models.py");

    assert!(init.contents.contains("AsyncApiClient"));
    assert!(init.contents.contains("ErrorHandler"));
    assert!(init.contents.contains("RequestOptions"));
    assert!(init.contents.contains("SyncApiClient"));
    assert!(models.contents.contains("from enum import Enum"));
    assert!(
        models
            .contents
            .contains("from typing import Any, TypeAlias")
    );
    assert!(models.contents.contains("WidgetPath: TypeAlias = \"str\""));
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
    assert!(client.contents.contains("import warnings"));
    assert!(client.contents.contains("from types import TracebackType"));
    assert!(client.contents.contains("from typing import Any, Self"));
    assert!(
        client
            .contents
            .contains("async def __aenter__(self) -> Self:")
    );
    assert!(client.contents.contains("def __enter__(self) -> Self:"));
    assert!(
        client
            .contents
            .contains("exc_type: type[BaseException] | None")
    );
    assert!(client.contents.contains("exc: BaseException | None"));
    assert!(client.contents.contains("tb: TracebackType | None"));
    assert!(
        client
            .contents
            .contains("from demo_client.utils import (")
    );
    assert!(utils.contents.contains("class RequestOptions(TypedDict, total=False):"));
    assert!(utils.contents.contains("on_error: ErrorHandler"));
    assert!(
        utils
            .contents
            .contains("raise TypeError(f\"{context} must be a base64 string\")")
    );
    assert!(
        client
            .contents
            .contains("on_error: ErrorHandler | None = None")
    );
    assert!(client.contents.contains("async def get_widget(self, widget_id:"));
    assert!(client.contents.contains("async def _get_widget_raw(self, widget_id:"));
    assert!(client.contents.contains("def get_widget(self, widget_id:"));
    assert!(client.contents.contains("def _get_widget_raw(self, widget_id:"));
    assert!(!client.contents.contains("def get_widget(self, *,"));
    assert!(
        client
            .contents
            .contains("url = f\"/widgets/{quote(str(widget_id), safe='')}\"")
    );
    assert!(
        client
            .contents
            .contains("warnings.warn(\"Endpoint `get_widget` is deprecated.\", DeprecationWarning, stacklevel=2)")
    );
    assert!(
        client
            .contents
            .contains("_suppress_deprecation_warning=True")
    );
    assert!(client.contents.contains("Args:"));
    assert!(
        client
            .contents
            .contains("widget_id: Unique widget identifier.")
    );
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
            .contains("request_kwargs = apply_request_options(request_kwargs, request_options, params=None, headers=None)")
    );
    assert!(
        utils
            .contents
            .contains("return body.model_dump(mode=\"json\", by_alias=True, exclude_unset=True)")
    );
    assert!(
        utils
            .contents
            .contains("return body.model_dump(by_alias=True, exclude_unset=True)")
    );
    assert!(
        client
            .contents
            .contains("handle_error(response, self._on_error, request_options)")
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
fn renders_keyword_only_operations_when_enabled() {
    let files = make_package(
        "demo_client",
        None,
        TargetConfig {
            keyword_only: true,
            ..Default::default()
        },
    )
    .expect("package should render");
    let client = files
        .iter()
        .find(|file| file.path.ends_with("client.py"))
        .expect("client.py");

    assert!(client.contents.contains("async def get_widget(self, *,"));
    assert!(client.contents.contains("def get_widget(self, *,"));
}

#[test]
fn renders_property_defaults_without_making_fields_nullable() {
    let ir = CoreIr {
        models: vec![arvalez_ir::Model {
            id: "model.my_schema".into(),
            name: "MySchema".into(),
            kind: arvalez_ir::ModelKind::Object,
            fields: vec![
                Field {
                    name: "iscool".into(),
                    type_ref: TypeRef::primitive("boolean"),
                    optional: true,
                    nullable: false,
                    attributes: Attributes::from([("default".into(), json!(false))]),
                },
                Field {
                    name: "maybe".into(),
                    type_ref: TypeRef::primitive("boolean"),
                    optional: false,
                    nullable: true,
                    attributes: Attributes::default(),
                },
                Field {
                    name: "wire-name".into(),
                    type_ref: TypeRef::primitive("boolean"),
                    optional: true,
                    nullable: false,
                    attributes: Attributes::from([("default".into(), json!(true))]),
                },
                Field {
                    name: "missing".into(),
                    type_ref: TypeRef::primitive("boolean"),
                    optional: true,
                    nullable: false,
                    attributes: Attributes::default(),
                },
            ],
            attributes: Attributes::default(),
            source: None,
        }],
        ..Default::default()
    };

    let files = make_package_from_ir(ir, "demo_client", None, TargetConfig::default())
        .expect("package should render");
    let models = files
        .iter()
        .find(|file| file.path.ends_with("models.py"))
        .expect("models.py");

    assert!(models.contents.contains("iscool: bool = False"));
    assert!(!models.contents.contains("iscool: bool | None"));
    assert!(models.contents.contains("maybe: bool | None\n"));
    assert!(models
        .contents
        .contains("wire_name: bool = Field(default=True, alias=\"wire-name\")"));
    assert!(models.contents.contains("missing: bool | None = None"));
}

#[test]
fn renders_multipart_file_requests() {
    let ir = CoreIr {
        models: vec![arvalez_ir::Model {
            id: "model.upload_body".into(),
            name: "UploadBody".into(),
            kind: arvalez_ir::ModelKind::Object,
            fields: vec![Field {
                name: "file".into(),
                type_ref: TypeRef::primitive("string"),
                optional: false,
                nullable: false,
                attributes: Attributes::from([(
                    "content_media_type".into(),
                    json!("application/octet-stream"),
                )]),
            }],
            attributes: Attributes::default(),
            source: None,
        }],
        operations: vec![Operation {
            id: "operation.upload_file".into(),
            name: "upload_file".into(),
            method: HttpMethod::Post,
            path: "/files".into(),
            params: Vec::new(),
            request_body: Some(RequestBody {
                required: true,
                media_type: "multipart/form-data".into(),
                type_ref: Some(TypeRef::named("UploadBody")),
                attributes: Attributes::default(),
            }),
            responses: vec![Response {
                status: "204".into(),
                media_type: None,
                type_ref: None,
                attributes: Attributes::default(),
            }],
            attributes: Attributes::default(),
            source: None,
        }],
        ..Default::default()
    };

    let files = make_package_from_ir(ir, "demo_client", None, TargetConfig::default())
        .expect("package should render");
    let client = files
        .iter()
        .find(|file| file.path.ends_with("client.py"))
        .expect("client.py");
    let models = files
        .iter()
        .find(|file| file.path.ends_with("models.py"))
        .expect("models.py");
    let utils = files
        .iter()
        .find(|file| file.path.ends_with("utils.py"))
        .expect("utils.py");

    assert!(models.contents.contains("file: bytes"));
    assert!(
        client
            .contents
            .contains("request_kwargs[\"files\"] = serialize_multipart_body(body)")
    );
    assert!(client.contents.contains("def upload_file(self, body: models.UploadBody"));
    assert!(utils.contents.contains("def serialize_multipart_body(body: Any) -> dict[str, Any]:"));
    assert!(utils.contents.contains("\"application/octet-stream\""));
}

#[test]
fn supports_selective_template_overrides() {
    let tempdir = tempdir().expect("tempdir");
    let partial_dir = tempdir.path().join("partials");
    fs::create_dir_all(&partial_dir).expect("partials dir");
    // The new template structure uses `class_name` (not `client.class_name`)
    fs::write(
        partial_dir.join("client_class.py.tera"),
        "class {{ class_name }}:\n    OVERRIDDEN = True\n",
    )
    .expect("override template");

    let files = make_package(
        "demo_client",
        Some(tempdir.path().to_path_buf()),
        TargetConfig::default(),
    )
    .expect("package should render");
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
    let files = make_package(
        "demo_client",
        None,
        TargetConfig {
            group_by_tag: true,
            ..Default::default()
        },
    )
    .expect("package should render");
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
    assert!(
        client
            .contents
            .contains("\n    async def _get_widget_raw")
    );
    assert!(client.contents.contains("\n    def _get_widget_raw"));
    assert!(client.contents.contains("async def healthcheck"));
    assert!(client.contents.contains("def healthcheck"));
}

#[test]
fn grouped_clients_use_readable_tag_subsection_names() {
    let ir = CoreIr {
        operations: vec![
            Operation {
                id: "operation.list_dags".into(),
                name: "list_dags".into(),
                method: HttpMethod::Get,
                path: "/dags".into(),
                params: Vec::new(),
                request_body: None,
                responses: vec![Response {
                    status: "200".into(),
                    media_type: None,
                    type_ref: None,
                    attributes: Attributes::default(),
                }],
                attributes: Attributes::from([("tags".into(), json!(["DAGs"]))]),
                source: None,
            },
            Operation {
                id: "operation.list_apis".into(),
                name: "list_apis".into(),
                method: HttpMethod::Get,
                path: "/apis".into(),
                params: Vec::new(),
                request_body: None,
                responses: vec![Response {
                    status: "200".into(),
                    media_type: None,
                    type_ref: None,
                    attributes: Attributes::default(),
                }],
                attributes: Attributes::from([("tags".into(), json!(["APIs"]))]),
                source: None,
            },
            Operation {
                id: "operation.list_headers".into(),
                name: "list_headers".into(),
                method: HttpMethod::Get,
                path: "/headers".into(),
                params: Vec::new(),
                request_body: None,
                responses: vec![Response {
                    status: "200".into(),
                    media_type: None,
                    type_ref: None,
                    attributes: Attributes::default(),
                }],
                attributes: Attributes::from([("tags".into(), json!(["HTTP Headers"]))]),
                source: None,
            },
        ],
        ..Default::default()
    };

    let files = make_package_from_ir(
        ir,
        "demo_client",
        None,
        TargetConfig {
            group_by_tag: true,
            ..Default::default()
        },
    )
    .expect("package should render");
    let client = files
        .iter()
        .find(|file| file.path.ends_with("client.py"))
        .expect("client.py");

    assert!(client.contents.contains("self.dags = AsyncDAGsApi(self)"));
    assert!(client.contents.contains("self.apis = AsyncAPIsApi(self)"));
    assert!(client
        .contents
        .contains("self.http_headers = AsyncHTTPHeadersApi(self)"));
    assert!(!client.contents.contains("self.da_gs"));
    assert!(!client.contents.contains("self.ap_is"));
}

#[test]
fn preserves_common_acronyms_in_python_names() {
    assert_eq!(sanitize_identifier("CreateAPIKey"), "create_api_key");
    assert_eq!(sanitize_identifier("AssociateWebACL"), "associate_web_acl");
    assert_eq!(sanitize_identifier("HTTPHeader"), "http_header");
    assert_eq!(
        sanitize_identifier("XAmzContentSHA256"),
        "x_amz_content_sha256"
    );
    assert_eq!(sanitize_identifier("UTF8String"), "utf8_string");
    assert_eq!(sanitize_identifier("IPv4Address"), "ipv4_address");
    assert_eq!(sanitize_class_name("APIKeySummary"), "APIKeySummary");
    assert_eq!(sanitize_class_name("WebACL"), "WebACL");
    assert_eq!(sanitize_class_name("HTTPHeader"), "HTTPHeader");
    assert_eq!(sanitize_class_name("SHA256Checksum"), "SHA256Checksum");
}

#[test]
fn sanitizes_tag_subsection_names_for_real_world_tag_shapes() {
    let cases = [
        ("DAGs", "dags"),
        ("APIs", "apis"),
        ("SDKs", "sdks"),
        ("ACLs", "acls"),
        ("IDs", "ids"),
        ("DAG", "dag"),
        ("DAG Runs", "dag_runs"),
        ("DAGs Enabled", "dags_enabled"),
        ("DAGStatus", "dag_status"),
        ("API Keys", "api_keys"),
        ("CreateAPIKey", "create_api_key"),
        ("HTTPHeader", "http_header"),
        ("HTTP Headers", "http_headers"),
        ("OAuth Apps", "oauth_apps"),
        ("OAuth2 Clients", "oauth2_clients"),
        ("GraphQL APIs", "graph_ql_apis"),
        ("IPv4 Address", "ipv4_address"),
        ("UTF8String", "utf8_string"),
        ("SHA256 Checksums", "sha256_checksums"),
        ("User Management", "user_management"),
        ("userManagement", "user_management"),
        ("user-management", "user_management"),
        ("user_management", "user_management"),
        ("  Admin API  ", "admin_api"),
        ("123 Reports", "_123_reports"),
        ("class", "class_"),
        ("", "value"),
    ];

    for (input, expected) in cases {
        assert_eq!(sanitize_subsection_name(input), expected, "{input}");
    }
}

#[test]
fn erases_default_template_with_tilde_prefix() {
    let dir = tempdir().expect("tempdir");
    // In the new structure, root templates are under `root/` in the template dir.
    let root_dir = dir.path().join("root");
    fs::create_dir_all(&root_dir).expect("root dir");

    // Place a tilde-prefixed eraser file to suppress pyproject.toml generation.
    fs::write(root_dir.join("~pyproject.toml.tera"), "").expect("eraser file");

    let files = make_package(
        "mylib",
        Some(dir.path().to_path_buf()),
        TargetConfig::default(),
    )
    .expect("package should render");

    // pyproject.toml must NOT be present in the output.
    assert!(
        !files
            .iter()
            .any(|f| f.path == std::path::PathBuf::from("pyproject.toml")),
        "pyproject.toml should be erased"
    );

    // All other default files should still be present.
    assert!(files.iter().any(|f| f.path.ends_with("README.md")));
    assert!(files.iter().any(|f| f.path.ends_with("client.py")));
    assert!(files.iter().any(|f| f.path.ends_with("models.py")));
    assert!(files.iter().any(|f| f.path.ends_with("utils.py")));
}

#[test]
fn renders_uuid_annotations_for_models_and_client_inputs() {
    let ir = CoreIr {
        models: vec![arvalez_ir::Model {
            id: "model.widget".into(),
            name: "Widget".into(),
            kind: arvalez_ir::ModelKind::Object,
            fields: vec![
                Field {
                    name: "id".into(),
                    type_ref: TypeRef::primitive("string"),
                    optional: false,
                    nullable: false,
                    attributes: Attributes::from([(
                        "format".into(),
                        Value::String("uuid4".into()),
                    )]),
                },
                Field {
                    name: "legacy_id".into(),
                    type_ref: TypeRef::primitive("string"),
                    optional: false,
                    nullable: false,
                    attributes: Attributes::from([(
                        "format".into(),
                        Value::String("uuid".into()),
                    )]),
                },
            ],
            attributes: Attributes::default(),
            source: None,
        }],
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
                    attributes: Attributes::from([(
                        "format".into(),
                        Value::String("uuid4".into()),
                    )]),
                }],
                request_body: None,
                responses: vec![Response {
                    status: "200".into(),
                    media_type: Some("application/json".into()),
                    type_ref: Some(TypeRef::primitive("string")),
                    attributes: Attributes::from([(
                        "format".into(),
                        Value::String("uuid4".into()),
                    )]),
                }],
                attributes: Attributes::default(),
                source: None,
            },
            Operation {
                id: "operation.create_widget".into(),
                name: "create_widget".into(),
                method: HttpMethod::Post,
                path: "/widgets".into(),
                params: Vec::new(),
                request_body: Some(RequestBody {
                    required: true,
                    media_type: "application/json".into(),
                    type_ref: Some(TypeRef::primitive("string")),
                    attributes: Attributes::from([(
                        "format".into(),
                        Value::String("uuid4".into()),
                    )]),
                }),
                responses: vec![Response {
                    status: "200".into(),
                    media_type: Some("application/json".into()),
                    type_ref: Some(TypeRef::named("Widget")),
                    attributes: Attributes::default(),
                }],
                attributes: Attributes::default(),
                source: None,
            },
        ],
        ..Default::default()
    };

    let files = make_package_from_ir(ir, "demo_client", None, TargetConfig::default())
        .expect("package should render");
    let client = files
        .iter()
        .find(|file| file.path.ends_with("client.py"))
        .expect("client.py");
    let models = files
        .iter()
        .find(|file| file.path.ends_with("models.py"))
        .expect("models.py");

    assert!(models.contents.contains("from uuid import UUID"));
    assert!(models.contents.contains("from pydantic import BaseModel, ConfigDict, Field, UUID4"));
    assert!(models.contents.contains("id: UUID4"));
    assert!(models.contents.contains("legacy_id: UUID"));

    assert!(client.contents.contains("from uuid import UUID"));
    assert!(client.contents.contains("widget_id: UUID | str"));
    assert!(client.contents.contains("def get_widget(self, widget_id: UUID | str, request_options: RequestOptions | None = None) -> UUID4:"));
    assert!(client.contents.contains("def create_widget(self, body: UUID | str, request_options: RequestOptions | None = None) -> models.Widget:"));
}

#[test]
fn stringifies_header_values_before_passing_them_to_httpx() {
    let ir = CoreIr {
        operations: vec![Operation {
            id: "operation.list_widgets".into(),
            name: "list_widgets".into(),
            method: HttpMethod::Get,
            path: "/widgets".into(),
            params: vec![
                Parameter {
                    name: "x-enabled".into(),
                    location: ParameterLocation::Header,
                    type_ref: TypeRef::primitive("boolean"),
                    required: true,
                    attributes: Attributes::default(),
                },
                Parameter {
                    name: "x-attempts".into(),
                    location: ParameterLocation::Header,
                    type_ref: TypeRef::primitive("integer"),
                    required: false,
                    attributes: Attributes::default(),
                },
            ],
            request_body: None,
            responses: vec![Response {
                status: "200".into(),
                media_type: Some("application/json".into()),
                type_ref: Some(TypeRef::primitive("string")),
                attributes: Attributes::default(),
            }],
            attributes: Attributes::default(),
            source: None,
        }],
        ..Default::default()
    };

    let files = make_package_from_ir(ir, "demo_client", None, TargetConfig::default())
        .expect("package should render");
    let utils = files
        .iter()
        .find(|file| file.path.ends_with("utils.py"))
        .expect("utils.py");

    assert!(
        utils
            .contents
            .contains("def stringify_header_value(value: Any) -> str:")
    );
    assert!(
        utils
            .contents
            .contains("merged_headers = stringify_headers(headers)")
    );
    assert!(
        utils
            .contents
            .contains("merged_headers.update(stringify_headers(request_options[\"headers\"]))")
    );
}
