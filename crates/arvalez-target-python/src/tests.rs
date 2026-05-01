use std::fs;

use serde_json::json;
use tempfile::tempdir;

use crate::sanitize::{sanitize_class_name, sanitize_identifier};
use crate::{CommonConfig, GeneratedFile, TargetConfig, generate};
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
    assert!(
        client
            .contents
            .contains("from demo_client.utils import (")
    );
    assert!(utils.contents.contains("class RequestOptions(TypedDict, total=False):"));
    assert!(utils.contents.contains("on_error: ErrorHandler"));
    assert!(
        client
            .contents
            .contains("on_error: ErrorHandler | None = None")
    );
    assert!(client.contents.contains("async def get_widget"));
    assert!(client.contents.contains("async def _get_widget_raw"));
    assert!(client.contents.contains("def get_widget"));
    assert!(client.contents.contains("def _get_widget_raw"));
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
    assert!(client.contents.contains("async def healthcheck"));
    assert!(client.contents.contains("def healthcheck"));
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
