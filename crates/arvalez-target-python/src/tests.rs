use std::fs;

use serde_json::json;
use tempfile::tempdir;

use crate::{PythonPackageConfig, generate_python_package};
use crate::sanitize::{sanitize_class_name, sanitize_identifier};
use arvalez_ir::{
    Attributes, CoreIr, Field, HttpMethod, Operation, ParameterLocation, Parameter, RequestBody,
    Response, TypeRef,
};
use serde_json::Value;

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
    let files = generate_python_package(&sample_ir(), &PythonPackageConfig::new("demo_client"))
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
    assert!(models.contents.contains("from typing import Any, TypeAlias"));
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
    let files = generate_python_package(&sample_ir(), &config).expect("package should render");
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
    let files = generate_python_package(&sample_ir(), &config).expect("package should render");
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
    assert_eq!(sanitize_identifier("XAmzContentSHA256"), "x_amz_content_sha256");
    assert_eq!(sanitize_identifier("UTF8String"), "utf8_string");
    assert_eq!(sanitize_identifier("IPv4Address"), "ipv4_address");
    assert_eq!(sanitize_class_name("APIKeySummary"), "APIKeySummary");
    assert_eq!(sanitize_class_name("WebACL"), "WebACL");
    assert_eq!(sanitize_class_name("HTTPHeader"), "HTTPHeader");
    assert_eq!(sanitize_class_name("SHA256Checksum"), "SHA256Checksum");
}
