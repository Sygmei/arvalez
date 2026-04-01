use std::fs;

use arvalez_ir::{Attributes, CoreIr, Field, HttpMethod, Operation, Parameter, ParameterLocation, RequestBody, Response, TypeRef};
use arvalez_target_core::{CommonConfig, PackageConfig};
use serde_json::{Value, json};
use tempfile::tempdir;

use crate::{TargetConfig, generate};

fn common(package_name: &str) -> CommonConfig {
    CommonConfig {
        package: PackageConfig {
            name: package_name.to_owned(),
            version: "0.1.0".into(),
            description: None,
        },
    }
}

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
                    attributes: Attributes::from([(
                        "description".into(),
                        Value::String("Unique widget identifier.".into()),
                    )]),
                },
                Parameter {
                    name: "include_count".into(),
                    location: ParameterLocation::Query,
                    type_ref: TypeRef::primitive("boolean"),
                    required: false,
                    attributes: Attributes::default(),
                },
            ],
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
        }],
        ..Default::default()
    }
}

#[test]
fn renders_basic_typescript_package() {
    let files = generate(&sample_ir(), None, &common("@demo/client"), &TargetConfig::default())
        .expect("package should render");

    let package_json = files
        .iter()
        .find(|file| file.path.ends_with("package.json"))
        .expect("package.json");
    let models = files
        .iter()
        .find(|file| file.path.ends_with("models.ts"))
        .expect("models.ts");
    let client = files
        .iter()
        .find(|file| file.path.ends_with("client.ts"))
        .expect("client.ts");
    let index = files
        .iter()
        .find(|file| file.path.ends_with("index.ts"))
        .expect("index.ts");

    assert!(package_json.contents.contains("\"name\": \"@demo/client\""));
    assert!(models.contents.contains("export interface Widget"));
    assert!(models.contents.contains("count?: number;"));
    assert!(client.contents.contains("export class ApiClient"));
    assert!(client.contents.contains("export interface RequestOptions"));
    assert!(
        client.contents.contains(
            "export type ErrorHandler = (response: globalThis.Response) => void | Promise<void>;"
        )
    );
    assert!(client.contents.contains("async _getWidgetRaw("));
    assert!(client.contents.contains("async getWidget("));
    assert!(
        client
            .contents
            .contains("@param widgetId Unique widget identifier.")
    );
    assert!(client.contents.contains("requestOptions?: RequestOptions"));
    assert!(client.contents.contains("onError?: ErrorHandler;"));
    assert!(
        client
            .contents
            .contains("const baseQuery = new URLSearchParams();")
    );
    assert!(
        client
            .contents
            .contains("const query = this.mergeQuery(baseQuery, requestOptions);")
    );
    assert!(client.contents.contains("body?: Widget"));
    assert!(
        client
            .contents
            .contains("const response = await this._getWidgetRaw(")
    );
    assert!(
        client
            .contents
            .contains("await this.handleError(response, requestOptions);")
    );
    assert!(
        index
            .contents
            .contains("export type { ApiClientOptions, ErrorHandler, RequestOptions }")
    );
}

#[test]
fn renders_aliases_and_enums_as_typescript_types() {
    let ir = CoreIr {
        models: vec![
            arvalez_ir::Model {
                id: "model.widget_path".into(),
                name: "WidgetPath".into(),
                fields: vec![],
                attributes: Attributes::from([(
                    "alias_type_ref".into(),
                    json!(TypeRef::primitive("string")),
                )]),
                source: None,
            },
            arvalez_ir::Model {
                id: "model.widget_status".into(),
                name: "WidgetStatus".into(),
                fields: vec![],
                attributes: Attributes::from([(
                    "enum_values".into(),
                    json!(["READY", "PAUSED"]),
                )]),
                source: None,
            },
        ],
        ..Default::default()
    };

    let files = generate(&ir, None, &common("@demo/client"), &TargetConfig::default())
        .expect("package should render");
    let models = files
        .iter()
        .find(|file| file.path.ends_with("models.ts"))
        .expect("models.ts");

    assert!(models.contents.contains("export type WidgetPath = string;"));
    assert!(models.contents.contains("export type WidgetStatus = \"READY\" | \"PAUSED\";"));
}

#[test]
fn groups_operations_by_tag_when_enabled() {
    let files = generate(
        &sample_ir(),
        None,
        &common("@demo/client"),
        &TargetConfig { group_by_tag: true },
    )
    .expect("package should render");
    let client = files
        .iter()
        .find(|file| file.path.ends_with("client.ts"))
        .expect("client.ts");

    assert!(client.contents.contains("readonly widgets = {"));
    assert!(
        client
            .contents
            .contains("getWidget: this.getWidget.bind(this),")
    );
    assert!(
        client
            .contents
            .contains("_getWidgetRaw: this._getWidgetRaw.bind(this),")
    );
}

#[test]
fn supports_selective_template_overrides() {
    let tempdir = tempdir().expect("tempdir");
    let partial_dir = tempdir.path().join("partials");
    fs::create_dir_all(&partial_dir).expect("partials dir");
    fs::write(
        partial_dir.join("tag_group.ts.tera"),
        "readonly {{ tag_group.property_name }} = { overridden: true };\n",
    )
    .expect("override template");

    let files = generate(
        &sample_ir(),
        Some(tempdir.path()),
        &common("@demo/client"),
        &TargetConfig { group_by_tag: true },
    )
    .expect("package should render");
    let client = files
        .iter()
        .find(|file| file.path.ends_with("client.ts"))
        .expect("client.ts");

    assert!(
        client
            .contents
            .contains("readonly widgets = { overridden: true };")
    );
}
