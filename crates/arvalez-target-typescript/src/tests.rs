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
            attributes: Attributes::from([
                ("tags".into(), json!(["widgets"])),
                ("deprecated".into(), json!(true)),
            ]),
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
    let utils = files
        .iter()
        .find(|file| file.path.ends_with("utils.ts"))
        .expect("utils.ts");
    let index = files
        .iter()
        .find(|file| file.path.ends_with("index.ts"))
        .expect("index.ts");

    assert!(package_json.contents.contains("\"name\": \"@demo/client\""));
    assert!(
        package_json
            .contents
            .contains("\"types\": \"./src/index.ts\"")
    );
    assert!(
        package_json
            .contents
            .contains("\"import\": \"./src/index.ts\"")
    );
    assert!(package_json.contents.contains("\"files\": [\n    \"src\"\n  ]"));
    assert!(!package_json.contents.contains("\"main\":"));
    assert!(!package_json.contents.contains("\"module\":"));
    assert!(!package_json.contents.contains("\"dist/index"));
    assert!(!package_json.contents.contains("\"prepack\":"));
    assert!(
        !package_json
            .contents
            .contains("\"typescript\": \"^5.0.0\"")
    );
    assert!(models.contents.contains("export interface Widget"));
    assert!(models.contents.contains("count?: number;"));
    assert!(client.contents.contains("export class ApiClient"));
    assert!(client.contents.contains("export type { ErrorHandler, RequestOptions } from \"./utils\";"));
    assert!(client.contents.contains("import type { ErrorHandler, RequestOptions } from \"./utils\";"));
    assert!(utils.contents.contains("export interface RequestOptions"));
    assert!(
        utils.contents.contains(
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
    assert!(client.contents.contains("@deprecated This operation is deprecated."));
    assert!(client.contents.contains("requestOptions?: RequestOptions"));
    assert!(utils.contents.contains("onError?: ErrorHandler;"));
    assert!(
        client
            .contents
            .contains("const baseQuery = new URLSearchParams();")
    );
    assert!(
        client
            .contents
            .contains("const query = mergeQuery(baseQuery, requestOptions);")
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
            .contains("await handleError(response, this.onError, requestOptions);")
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
                kind: arvalez_ir::ModelKind::Object,
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
                kind: arvalez_ir::ModelKind::Enum {
                    base: TypeRef::primitive("string"),
                    values: vec![json!("READY"), json!("PAUSED")],
                },
                fields: vec![],
                attributes: Attributes::default(),
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
    assert!(
        models
            .contents
            .contains("export const WidgetStatusValues = [\"READY\",\"PAUSED\"] as const;")
    );
    assert!(
        models
            .contents
            .contains("export type WidgetStatus = (typeof WidgetStatusValues)[number];")
    );
}

#[test]
fn renders_inline_enum_values_as_runtime_tuples_and_derived_types() {
    let values = vec![json!("DMZRC"), json!("MZRC"), json!("ONPREM")];
    let mut evaluate_request = arvalez_ir::Model::new("model.evaluate_request", "EvaluateRequest");
    evaluate_request.fields.push(Field {
        name: "account_category".into(),
        type_ref: TypeRef::enumeration(
            "EvaluateRequestAccountCategory",
            TypeRef::primitive("string"),
            values.clone(),
        ),
        optional: true,
        nullable: true,
        attributes: Attributes::default(),
    });
    let mut rule_filter = arvalez_ir::Model::new("model.rule_filter", "RuleFilter");
    rule_filter.fields.push(Field {
        name: "account_category".into(),
        type_ref: TypeRef::array(TypeRef::enumeration(
            "RuleFilterAccountCategory",
            TypeRef::primitive("string"),
            values,
        )),
        optional: true,
        nullable: true,
        attributes: Attributes::default(),
    });
    let ir = CoreIr {
        models: vec![evaluate_request, rule_filter],
        ..Default::default()
    };

    let files = generate(&ir, None, &common("@demo/client"), &TargetConfig::default())
        .expect("package should render");
    let models = files
        .iter()
        .find(|file| file.path.ends_with("models.ts"))
        .expect("models.ts");
    let index = files
        .iter()
        .find(|file| file.path.ends_with("index.ts"))
        .expect("index.ts");

    assert!(models.contents.contains(
        "export const EvaluateRequestAccountCategoryValues = [\"DMZRC\",\"MZRC\",\"ONPREM\"] as const;"
    ));
    assert!(models.contents.contains(
        "export type EvaluateRequestAccountCategory = (typeof EvaluateRequestAccountCategoryValues)[number];"
    ));
    assert!(models.contents.contains(
        "account_category?: EvaluateRequestAccountCategory | null;"
    ));
    assert!(models.contents.contains(
        "account_category?: RuleFilterAccountCategory[] | null;"
    ));
    assert!(index.contents.contains("export * from \"./models\";"));
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
