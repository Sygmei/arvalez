use crate::config::{
    AppConfig, normalize_go_package_name, normalize_python_package_name, resolve_go_config,
    resolve_python_config,
};
use crate::corpus::classify_failure;

use arvalez_ir::{Attributes, CoreIr, Operation};
use serde_json::json;

#[test]
fn normalize_python_package_name_replaces_hyphens() {
    assert_eq!(
        normalize_python_package_name("arvalez-client"),
        "arvalez_client"
    );
}

#[test]
fn resolve_python_config_normalizes_module_name() {
    let config = AppConfig::default();
    let (common, _, _) = resolve_python_config(
        &config,
        Some("arvalez-client".into()),
        None,
        false,
        false,
        Some("0.1.0".into()),
    );

    assert_eq!(common.package.name, "arvalez_client");
}

#[test]
fn resolves_python_keyword_only_setting_from_config() {
    let config: AppConfig = toml::from_str(
        r#"
[target.python]
keyword_only = true
"#,
    )
    .expect("config should parse");
    let (_, target, _) = resolve_python_config(&config, None, None, false, false, None);

    assert!(target.keyword_only);
}

#[test]
fn normalize_go_package_name_replaces_hyphens() {
    assert_eq!(normalize_go_package_name("arvalez-client"), "arvalezclient");
}

#[test]
fn resolve_go_config_normalizes_explicit_package_name() {
    let config = AppConfig::default();
    let (common, _, _) = resolve_go_config(
        &config,
        Some("github.com/acme/arvalez-client".into()),
        Some("arvalez-client".into()),
        None,
        false,
        Some("0.1.0".into()),
    );

    assert_eq!(common.package.name, "arvalezclient");
}

#[test]
fn target_output_overrides_common_skip_deprecated_operations() {
    let config: AppConfig = toml::from_str(
        r#"
[common.output]
skip_deprecated_operations = true

[target.python.output]
skip_deprecated_operations = false
"#,
    )
    .expect("config should parse");

    assert!(
        config
            .target
            .go
            .base
            .resolve_skip_deprecated_operations(&config.common.output)
    );
    assert!(
        !config
            .target
            .python
            .resolve_skip_deprecated_operations(&config.common.output)
    );
}

#[test]
fn filters_deprecated_operations_when_enabled() {
    let ir = CoreIr {
        operations: vec![
            Operation {
                name: "active".into(),
                ..Default::default()
            },
            Operation {
                name: "old".into(),
                attributes: Attributes::from([("deprecated".into(), json!(true))]),
                ..Default::default()
            },
        ],
        ..Default::default()
    };

    let filtered = arvalez_target_core::prepare_generation_ir(
        &ir,
        &arvalez_target_core::CommonConfig {
            skip_deprecated_operations: true,
            ..Default::default()
        },
        arvalez_target_core::ModelTarget::Go,
        arvalez_target_go::model_identifier,
    )
    .unwrap();
    assert_eq!(filtered.operations.len(), 1);
    assert_eq!(filtered.operations[0].name, "active");

    let unfiltered = arvalez_target_core::prepare_generation_ir(
        &ir,
        &arvalez_target_core::CommonConfig {
            skip_deprecated_operations: false,
            ..Default::default()
        },
        arvalez_target_core::ModelTarget::Go,
        arvalez_target_go::model_identifier,
    )
    .unwrap();
    assert_eq!(unfiltered.operations.len(), 2);
}

#[test]
fn corpus_uses_model_policies_before_filtering_deprecated_operations() {
    use crate::corpus::{CorpusTestOptions, run_corpus_spec_inline};
    let dir = tempfile::tempdir().unwrap();
    let spec = dir.path().join("openapi.json");
    let mut document: serde_json::Value =
        serde_json::from_str(include_str!("../tests/fixtures/response_fields.json")).unwrap();
    document["paths"]["/widgets"]["post"]["deprecated"] = json!(true);
    std::fs::write(&spec, document.to_string()).unwrap();
    let config: AppConfig = toml::from_str("[common.models]\nresponse_fields='always_present'\n[common.output]\nskip_deprecated_operations=true\n[target.typescript.models]\nresponse_fields='schema'").unwrap();
    let output = dir.path().join("out");
    let options = CorpusTestOptions {
        config: dir.path().join("arvalez.toml"),
        repository: String::new(),
        reference: String::new(),
        checkout_directory: None,
        output_directory: Some(output.clone()),
        report_directory: None,
        ignore_unhandled: false,
        no_go: false,
        no_python: false,
        no_typescript: false,
        no_nushell: false,
        output_version: None,
        limit: None,
        jobs: None,
        ui: false,
    };
    let report = run_corpus_spec_inline(&config, &spec, "openapi.json", &options);
    assert!(report.failure.is_none(), "{:?}", report.failure);
    assert_eq!(report.targets.len(), 4);
    for target in report.targets {
        assert!(target.failure.is_none(), "{:?}", target.failure);
    }
    let go = std::fs::read_to_string(output.join("go-client/models.go")).unwrap();
    assert!(go.contains("type WidgetResponse struct"));
    assert!(go.contains("Nickname *string `json:\"nickname\"`"));
    let ts = std::fs::read_to_string(output.join("typescript-client/src/models.ts")).unwrap();
    assert!(!ts.contains("interface WidgetResponse"));
    let client = std::fs::read_to_string(output.join("go-client/client.go")).unwrap();
    assert!(!client.contains("func (c *Client) CreateWidget("));
}

#[test]
fn classifies_empty_request_body_content() {
    let failure = classify_failure(
        "OpenAPI document issue\nCaused by:\n  request body has no content entries\n  location: #/paths/~1widgets/post/requestBody/content\n  note: Arvalez expects at least one media type under `requestBody.content`.",
        None,
    );
    assert_eq!(failure.kind, "unsupported_request_body_shape");
    assert_eq!(failure.feature, "empty_content");
    assert_eq!(
        failure.pointer.as_deref(),
        Some("#/paths/~1widgets/post/requestBody/content")
    );
}

#[test]
fn classifies_incompatible_all_of_declarations() {
    let failure = classify_failure(
        "OpenAPI document issue\nCaused by:\n  `allOf` contains incompatible `title` declarations\n  location: #/components/schemas/Foo\n  preview:\n    allOf:\n    - $ref: '#/components/schemas/Bar'\n  note: Use `--ignore-unhandled` to turn this into a warning while keeping generation going.",
        None,
    );
    assert_eq!(failure.kind, "unsupported_all_of_merge");
    assert_eq!(failure.feature, "title");
    assert_eq!(failure.pointer.as_deref(), Some("#/components/schemas/Foo"));
}

#[test]
fn classifies_parameter_missing_schema() {
    let failure = classify_failure(
        "parameter `x-apideck-metadata`: parameter has no schema or type\nnote: Arvalez currently expects non-body parameters to declare either `schema` (OpenAPI 3) or `type` (Swagger 2).",
        None,
    );
    assert_eq!(failure.kind, "invalid_openapi_document");
    assert_eq!(failure.feature, "x-apideck-metadata");
}

#[test]
fn classifies_parameter_with_empty_name_value() {
    let failure = classify_failure(
        "OpenAPI document issue\nCaused by:\n  parameter #1 has an empty name\n  location: #/paths/~1customers/get\n  note: Use `--ignore-unhandled` to turn this into a warning while keeping generation going.",
        None,
    );
    assert_eq!(failure.kind, "invalid_openapi_document");
    assert_eq!(failure.feature, "empty_parameter_name");
    assert_eq!(failure.pointer.as_deref(), Some("#/paths/~1customers/get"));
}

#[test]
fn classifies_property_with_empty_key() {
    let failure = classify_failure(
        "OpenAPI document issue\nCaused by:\n  property #1 has an empty name\n  location: #/components/schemas/shared-user/properties\n  preview:\n    '':\n      type: string\n    username:\n      type: string\n  note: Use `--ignore-unhandled` to turn this into a warning while keeping generation going.",
        None,
    );
    assert_eq!(failure.kind, "invalid_openapi_document");
    assert_eq!(failure.feature, "empty_property_key");
    assert_eq!(
        failure.pointer.as_deref(),
        Some("#/components/schemas/shared-user/properties")
    );
}

#[test]
fn classifies_array_missing_items() {
    let failure = classify_failure(
        "OpenAPI document issue\nCaused by:\n  array schema is missing `items`\n  location: #/definitions/DBResp/properties/Data\n  preview:\n    example:\n    - <array of data objects>\n    type: array\n  note: Add an `items` schema to describe the array element type.",
        None,
    );
    assert_eq!(failure.kind, "invalid_openapi_document");
    assert_eq!(failure.feature, "Data");
    assert_eq!(
        failure.pointer.as_deref(),
        Some("#/definitions/DBResp/properties/Data")
    );
}

#[test]
fn parses_pagination_defaults_and_operation_overrides() {
    let config: AppConfig = toml::from_str(
        r#"
[common.pagination]
strategy = "page"
page_param = "query.page"
page_size_param = "query.limit"
start_page = 1
default_page_size = 100
max_page_size = 500
items_path = "/items"
total_pages_path = "/pages"
[common.pagination.operations.GetAllEvents]
start_page = 0
default_page_size = 50
max_page_size = 100
[common.pagination.operations.GetAllCountries]
disabled = true
"#,
    )
    .unwrap();
    let (common, _, _) = resolve_go_config(&config, None, None, None, false, None);
    let events = common
        .pagination
        .defaults
        .with_override(&common.pagination.operations["GetAllEvents"]);
    assert_eq!(events.start_page, Some(0));
    assert_eq!(events.max_page_size, Some(100));
    assert_eq!(events.items_path.as_deref(), Some("/items"));
    assert_eq!(
        common.pagination.operations["GetAllCountries"].disabled,
        Some(true)
    );
}

#[test]
fn rejects_unknown_pagination_settings() {
    for input in [
        "[common.pagination]\nstart_pgae = 0",
        "[common.pagination.operations.GetAllEvents]\nmax_size = 10",
    ] {
        assert!(toml::from_str::<AppConfig>(input).is_err(), "{input}");
    }
}
