use crate::config::{AppConfig, normalize_python_package_name, resolve_python_config};
use crate::corpus::classify_failure;

#[test]
fn normalize_python_package_name_replaces_hyphens() {
    assert_eq!(normalize_python_package_name("arvalez-client"), "arvalez_client");
}

#[test]
fn resolve_python_config_normalizes_module_name() {
    let config = AppConfig::default();
    let (common, _, _) = resolve_python_config(
        &config,
        Some("arvalez-client".into()),
        None,
        false,
        Some("0.1.0".into()),
    );

    assert_eq!(common.package.name, "arvalez_client");
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
