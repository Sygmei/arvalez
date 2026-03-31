use arvalez_ir::{
    Attributes, CoreIr, Field, HttpMethod, Operation, Parameter, ParameterLocation, RequestBody,
    Response, TypeRef,
};
use serde_json::{Value, json};

use crate::{NushellPackageConfig, generate_nushell_package};

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
            request_body: None,
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

fn post_ir() -> CoreIr {
    CoreIr {
        models: vec![],
        operations: vec![Operation {
            id: "operation.create_widget".into(),
            name: "create_widget".into(),
            method: HttpMethod::Post,
            path: "/widgets".into(),
            params: vec![],
            request_body: Some(RequestBody {
                required: true,
                media_type: "application/json".into(),
                type_ref: None,
                attributes: Attributes::default(),
            }),
            responses: vec![Response {
                status: "201".into(),
                media_type: Some("application/json".into()),
                type_ref: None,
                attributes: Attributes::default(),
            }],
            attributes: Attributes::default(),
            source: None,
        }],
        ..Default::default()
    }
}

#[test]
fn renders_basic_nushell_package() {
    let config = NushellPackageConfig::new("my-api")
        .with_version("1.0.0")
        .with_default_base_url("https://api.example.com");

    let files = generate_nushell_package(&sample_ir(), &config).expect("should render");

    let paths: Vec<_> = files.iter().map(|f| f.path.to_str().unwrap()).collect();
    assert!(paths.contains(&"README.md"), "README.md missing");
    assert!(paths.contains(&"mod.nu"), "mod.nu missing");
    assert!(paths.contains(&"client.nu"), "client.nu missing");
    assert!(paths.contains(&"models.nu"), "models.nu missing");
}

#[test]
fn client_contains_command_and_path_param() {
    let config = NushellPackageConfig::new("my-api");
    let files = generate_nushell_package(&sample_ir(), &config).expect("should render");

    let client = files
        .iter()
        .find(|f| f.path.ends_with("client.nu"))
        .expect("client.nu");

    assert!(
        client.contents.contains("get-widget"),
        "command name not found"
    );
    assert!(
        client.contents.contains("widget_id"),
        "path param not found"
    );
    assert!(
        client.contents.contains("http get"),
        "http verb not found"
    );
}

#[test]
fn models_contains_make_command() {
    let config = NushellPackageConfig::new("my-api");
    let files = generate_nushell_package(&sample_ir(), &config).expect("should render");

    let models = files
        .iter()
        .find(|f| f.path.ends_with("models.nu"))
        .expect("models.nu");

    assert!(
        models.contents.contains("make-widget"),
        "model constructor not found: {}",
        models.contents
    );
}

#[test]
fn post_command_includes_body() {
    let config = NushellPackageConfig::new("my-api");
    let files = generate_nushell_package(&post_ir(), &config).expect("should render");

    let client = files
        .iter()
        .find(|f| f.path.ends_with("client.nu"))
        .expect("client.nu");

    assert!(
        client.contents.contains("create-widget"),
        "command not found"
    );
    assert!(
        client.contents.contains("--body"),
        "--body flag not found in: {}",
        client.contents
    );
    assert!(client.contents.contains("http post"), "http post not found");
}
#[test]
fn models_use_typed_record_return() {
    let config = NushellPackageConfig::new("my-api");
    let files = generate_nushell_package(&sample_ir(), &config).expect("should render");

    let models = files
        .iter()
        .find(|f| f.path.ends_with("models.nu"))
        .expect("models.nu");

    // The Widget model has id: string and count: int, so the constructor should
    // return a typed record rather than the bare `record` annotation.
    assert!(
        models.contents.contains("record<"),
        "typed record annotation not found in: {}",
        models.contents
    );
    assert!(
        models.contents.contains("id: string"),
        "typed field id: string not found in: {}",
        models.contents
    );
}

#[test]
fn group_by_tag_prefixes_command_name() {
    let config = NushellPackageConfig::new("my-api").with_group_by_tag(true);
    let files = generate_nushell_package(&sample_ir(), &config).expect("should render");

    let client = files
        .iter()
        .find(|f| f.path.ends_with("client.nu"))
        .expect("client.nu");

    // The get_widget operation has tag "widgets", so with group_by_tag the
    // export should be `"widgets get-widget"`.
    assert!(
        client.contents.contains("widgets get-widget"),
        "tagged subcommand not found in: {}",
        client.contents
    );
}

#[test]
fn no_group_by_tag_uses_flat_command_name() {
    let config = NushellPackageConfig::new("my-api");
    let files = generate_nushell_package(&sample_ir(), &config).expect("should render");

    let client = files
        .iter()
        .find(|f| f.path.ends_with("client.nu"))
        .expect("client.nu");

    // Without group_by_tag the command should just be `"get-widget"`.
    assert!(
        client.contents.contains("\"get-widget\""),
        "flat command name not found in: {}",
        client.contents
    );
    assert!(
        !client.contents.contains("\"widgets get-widget\""),
        "tag prefix should not appear without group_by_tag"
    );
}