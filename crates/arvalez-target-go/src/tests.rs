use std::collections::BTreeMap;
use std::fs;

use arvalez_ir::{
    Attributes, CoreIr, Field, HttpMethod, Operation, Parameter, ParameterLocation, RequestBody,
    Response, TypeRef,
};
use serde_json::{Value, json};
use tempfile::tempdir;

use crate::{GoPackageConfig, generate_go_package};

fn sample_ir() -> CoreIr {
    CoreIr {
        models: vec![arvalez_ir::Model {
            id: "model.widget".into(),
            name: "Widget".into(),
            fields: vec![
                Field::new("id", TypeRef::primitive("string")),
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
                    attributes: BTreeMap::new(),
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
            attributes: Attributes::from([("tags".into(), json!(["widgets"]))]),
            source: None,
        }],
        ..Default::default()
    }
}

#[test]
fn renders_basic_go_package() {
    let files = generate_go_package(
        &sample_ir(),
        &GoPackageConfig::new("github.com/demo/client"),
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

    assert!(go_mod.contents.contains("module github.com/demo/client"));
    assert!(models.contents.contains("type Widget struct"));
    assert!(models.contents.contains("Count *int64 `json:\"count,omitempty\"`"));
    assert!(client.contents.contains("type ErrorHandler func(*http.Response) error"));
    assert!(client.contents.contains("type RequestOptions struct"));
    assert!(client.contents.contains("func (c *Client) GetWidgetRaw("));
    assert!(client.contents.contains("func (c *Client) GetWidget("));
    assert!(client.contents.contains("GetWidget parameter widgetId: Unique widget identifier."));
    assert!(client.contents.contains("requestOptions *RequestOptions"));
    assert!(client.contents.contains("if err := client.handleError(response, requestOptions); err != nil {"));
    assert!(client.contents.contains("response, err := c.GetWidgetRaw("));
}

#[test]
fn groups_operations_by_tag_when_enabled() {
    let files = generate_go_package(
        &sample_ir(),
        &GoPackageConfig::new("github.com/demo/client").with_group_by_tag(true),
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
        &GoPackageConfig::new("github.com/demo/client")
            .with_group_by_tag(true)
            .with_template_dir(Some(tempdir.path().to_path_buf())),
    )
    .expect("package should render");
    let client = files
        .iter()
        .find(|file| file.path.ends_with("client.go"))
        .expect("client.go");

    assert!(client.contents.contains("type WidgetsService struct { Overridden bool }"));
}
