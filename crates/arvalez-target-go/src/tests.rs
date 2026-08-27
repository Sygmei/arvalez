use std::collections::BTreeMap;
use std::{fs, process::Command};

use arvalez_ir::{
    Attributes, CoreIr, Field, HttpMethod, Operation, Parameter, ParameterLocation, RequestBody,
    Response, TypeRef,
};
use arvalez_target_core::{CommonConfig, PackageConfig};
use serde_json::{Value, json};
use tempfile::tempdir;

use crate::{TargetConfig, generate_go_package, write_go_package};

fn sample_ir() -> CoreIr {
    CoreIr {
        models: vec![arvalez_ir::Model {
            id: "model.widget".into(),
            name: "Widget".into(),
            kind: arvalez_ir::ModelKind::Object,
            fields: vec![
                Field::new("id", TypeRef::primitive("string")),
                Field {
                    name: "count".into(),
                    type_ref: TypeRef::primitive("integer"),
                    optional: true,
                    nullable: false,
                    attributes: Attributes::default(),
                },
                Field {
                    name: "picture_url".into(),
                    type_ref: TypeRef::primitive("string"),
                    optional: true,
                    nullable: true,
                    attributes: Attributes::default(),
                },
            ],
            attributes: Attributes::default(),
            source: None,
        }],
        operations: vec![Operation {
            id: "operation.get_widget".into(),
            name: "get_widget".into(),
            method: HttpMethod::Get,
            path: "/widgets/{widget_id}".into(),
            params: vec![
                Parameter {
                    name: "widget_id".into(),
                    location: ParameterLocation::Path,
                    type_ref: TypeRef::primitive("string"),
                    required: true,
                    attributes: BTreeMap::from([(
                        "description".into(),
                        Value::String("Unique widget identifier.".into()),
                    )]),
                },
                Parameter {
                    name: "include_count".into(),
                    location: ParameterLocation::Query,
                    type_ref: TypeRef::primitive("boolean"),
                    required: false,
                    attributes: BTreeMap::from([(
                        "description".into(),
                        Value::String("Whether to include the total count.".into()),
                    )]),
                },
            ],
            request_body: Some(RequestBody {
                required: false,
                media_type: "application/json".into(),
                type_ref: Some(TypeRef::named("Widget")),
                attributes: BTreeMap::new(),
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
        }],
        ..Default::default()
    }
}

fn default_common() -> CommonConfig {
    CommonConfig { package: PackageConfig { name: "client".into(), version: "0.1.0".into(), description: None } }
}

#[test]
fn renders_basic_go_package() {
    let files = generate_go_package(
        &sample_ir(),
        None,
        &default_common(),
        &TargetConfig { module_path: "github.com/demo/client".into(), ..Default::default() },
    )
    .expect("package should render");

    let go_mod = files
        .iter()
        .find(|file| file.path.ends_with("go.mod"))
        .expect("go.mod");
    let models = files
        .iter()
        .find(|file| file.path.ends_with("models.go"))
        .expect("models.go");
    let client = files
        .iter()
        .find(|file| file.path.ends_with("client.go"))
        .expect("client.go");
    let utils = files
        .iter()
        .find(|file| file.path.ends_with("utils.go"))
        .expect("utils.go");

    assert!(go_mod.contents.contains("module github.com/demo/client"));
    assert!(models.contents.contains("type Widget struct"));
    assert!(models.contents.contains("Count *int64 `json:\"count,omitempty\"`"));
    assert!(models.contents.contains("PictureURL *Nullable[string] `json:\"picture_url,omitempty\"`"));
    assert!(models.contents.contains("type Nullable[T any] struct"));
    assert!(models.contents.contains("func Null[T any]() *Nullable[T]"));
    assert!(client.contents.contains("type ErrorHandler func(*http.Response) error"));
    assert!(client.contents.contains("type RequestOptions struct"));
    assert!(!client.contents.contains("Context                  context.Context"));
    assert!(utils.contents.contains("type APIError struct"));
    assert!(utils.contents.contains("Body       []byte"));
    assert!(!utils.contents.contains("func (c *Client) resolveContext("));
    assert!(utils.contents.contains("func (c *Client) encodeMultipartBody(payload any) (io.Reader, string, error) {"));
    assert!(client.contents.contains("func (c *Client) GetWidgetRaw("));
    assert!(client.contents.contains("func (c *Client) GetWidget("));
    assert!(client.contents.contains("body *Widget"));
    assert!(client.contents.contains("// Deprecated: This operation is deprecated."));
    assert!(client.contents.contains("GetWidget parameter widgetID: Unique widget identifier."));
    assert!(client.contents.contains("requestOptions *RequestOptions"));
    assert!(client.contents.contains("if err := client.handleError(response, requestOptions); err != nil {"));
    assert!(client.contents.contains("response, err := c.GetWidgetRaw("));
    assert!(client.contents.contains(
        "// GetWidgetRaw parameter widgetID: Unique widget identifier.\n// GetWidgetRaw parameter includeCount: Whether to include the total count.\nfunc (c *Client) GetWidgetRaw("
    ));
    assert!(client.contents.contains(
        "// GetWidget parameter widgetID: Unique widget identifier.\n// GetWidget parameter includeCount: Whether to include the total count.\nfunc (c *Client) GetWidget("
    ));
    assert!(client.contents.contains(
        "if includeCount != nil {\n\t\tquery.Set(\"include_count\", fmt.Sprint(*includeCount))\n\t}"
    ));
}

#[test]
fn generated_package_builds_and_vets_when_go_is_available() {
    if Command::new("go").arg("version").output().is_err() {
        return;
    }

    let output_dir = tempdir().expect("tempdir");
    let files = generate_go_package(
        &sample_ir(),
        None,
        &default_common(),
        &TargetConfig { module_path: "github.com/demo/client".into(), ..Default::default() },
    )
    .expect("package should render");
    write_go_package(output_dir.path(), &files).expect("package should be written");
    let go_cache = output_dir.path().join("go-cache");
    fs::create_dir_all(&go_cache).expect("Go cache directory should be created");

    for command in ["test", "vet"] {
        let output = Command::new("go")
            .args([command, "./..."])
            .current_dir(output_dir.path())
            .env("GOCACHE", &go_cache)
            .output()
            .expect("go command should run");
        assert!(
            output.status.success(),
            "go {command} failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn avoids_collisions_between_parameters_and_generated_locals() {
    let mut ir = sample_ir();
    ir.operations[0].params[1].name = "query".into();

    let files = generate_go_package(&ir, None, &default_common(), &TargetConfig::default())
        .expect("package should render");
    let client = files
        .iter()
        .find(|file| file.path.ends_with("client.go"))
        .expect("client.go");

    assert!(client.contents.contains("query *bool"));
    assert!(client.contents.contains("query_.Set(\"query\", fmt.Sprint(*query))"));
    assert!(client.contents.contains("query_ := url.Values{}"));
}

#[test]
fn required_named_request_bodies_are_values() {
    let mut ir = sample_ir();
    ir.operations[0].request_body.as_mut().expect("request body").required = true;

    let files = generate_go_package(
        &ir,
        None,
        &default_common(),
        &TargetConfig::default(),
    )
    .expect("package should render");
    let client = files
        .iter()
        .find(|file| file.path.ends_with("client.go"))
        .expect("client.go");

    assert!(client.contents.contains("body Widget"));
    assert!(!client.contents.contains("body *Widget"));
}

#[test]
fn preserves_common_go_initialisms() {
    assert_eq!(crate::sanitize::sanitize_exported_identifier("picture_url"), "PictureURL");
    assert_eq!(crate::sanitize::sanitize_exported_identifier("get_item_by_id"), "GetItemByID");
    assert_eq!(crate::sanitize::sanitize_identifier("item_id"), "itemID");
    assert_eq!(crate::sanitize::sanitize_identifier("x_admin_token"), "xAdminToken");
}

#[test]
fn groups_operations_by_tag_when_enabled() {
    let files = generate_go_package(
        &sample_ir(),
        None,
        &default_common(),
        &TargetConfig { module_path: "github.com/demo/client".into(), group_by_tag: true },
    )
    .expect("package should render");
    let client = files
        .iter()
        .find(|file| file.path.ends_with("client.go"))
        .expect("client.go");

    assert!(client.contents.contains("Widgets *WidgetsService"));
    assert!(client.contents.contains("client.Widgets = &WidgetsService{client: client}"));
    assert!(client.contents.contains("type WidgetsService struct"));
    assert!(client.contents.contains("func (s *WidgetsService) GetWidgetRaw("));
}

#[test]
fn supports_selective_template_overrides() {
    let tempdir = tempdir().expect("tempdir");
    let partial_dir = tempdir.path().join("partials");
    fs::create_dir_all(&partial_dir).expect("partials dir");
    fs::write(
        partial_dir.join("service.go.tera"),
        "type {{ service.struct_name }} struct { Overridden bool }\n",
    )
    .expect("override template");

    let files = generate_go_package(
        &sample_ir(),
        Some(tempdir.path()),
        &default_common(),
        &TargetConfig { module_path: "github.com/demo/client".into(), group_by_tag: true },
    )
    .expect("package should render");
    let client = files
        .iter()
        .find(|file| file.path.ends_with("client.go"))
        .expect("client.go");

    assert!(client.contents.contains("type WidgetsService struct { Overridden bool }"));
}
