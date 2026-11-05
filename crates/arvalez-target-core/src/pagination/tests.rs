use super::*;
use arvalez_ir::{Field, Model, Parameter, Response};
use serde_json::json;

fn fixture() -> (CoreIr, PaginationConfig) {
    let mut page = Model::new("page", "Page");
    page.fields = vec![
        Field::new("items", TypeRef::array(TypeRef::primitive("string"))),
        Field::new("pages", TypeRef::primitive("integer")),
        Field::new("next", TypeRef::primitive("string")),
        Field::new("more", TypeRef::primitive("boolean")),
    ];
    let op = Operation {
        id: "operation.GetAllWidgets".into(),
        name: "GetAllWidgets".into(),
        params: ["page", "limit", "offset", "cursor"]
            .into_iter()
            .map(|name| Parameter {
                name: name.into(),
                location: ParameterLocation::Query,
                type_ref: TypeRef::primitive(if name == "cursor" {
                    "string"
                } else {
                    "integer"
                }),
                required: false,
                attributes: Default::default(),
            })
            .collect(),
        responses: vec![Response {
            status: "200".into(),
            media_type: Some("application/json".into()),
            type_ref: Some(TypeRef::named("Page")),
            attributes: Default::default(),
        }],
        ..Default::default()
    };
    let config = serde_json::from_value(json!({"strategy":"page", "page_param":"query.page",
        "page_size_param":"query.limit", "default_page_size":100, "max_page_size":500,
        "items_path":"/items", "total_pages_path":"/pages"}))
    .unwrap();
    (
        CoreIr {
            models: vec![page],
            operations: vec![op],
            ..Default::default()
        },
        config,
    )
}

#[test]
fn resolves_defaults_and_route_overrides() {
    let (ir, mut config) = fixture();
    let resolved = resolve_pagination(&ir, &config).unwrap();
    assert_eq!(resolved["operation.GetAllWidgets"].start, 1);
    config.operations.insert(
        "GetAllWidgets".into(),
        PaginationSettings {
            start_page: Some(0),
            default_page_size: Some(50),
            max_page_size: Some(100),
            ..Default::default()
        },
    );
    let resolved = resolve_pagination(&ir, &config).unwrap();
    let pg = &resolved["operation.GetAllWidgets"];
    assert_eq!(pg.start, 0);
    assert_eq!(pg.settings.max_page_size, Some(100));
    assert_eq!(pg.item_type, TypeRef::primitive("string"));
    config.operations.get_mut("GetAllWidgets").unwrap().disabled = Some(true);
    assert!(resolve_pagination(&ir, &config).unwrap().is_empty());
}

#[test]
fn changing_strategy_and_termination_replaces_inherited_fields() {
    let (ir, mut config) = fixture();
    config.operations.insert(
        "GetAllWidgets".into(),
        PaginationSettings {
            strategy: Some(PaginationStrategy::Cursor),
            cursor_param: Some("query.cursor".into()),
            next_cursor_path: Some("/next".into()),
            ..Default::default()
        },
    );
    let resolved = resolve_pagination(&ir, &config).unwrap();
    let pg = &resolved["operation.GetAllWidgets"];
    assert!(pg.settings.page_param.is_none());
    assert!(pg.settings.total_pages_path.is_none());
    assert_eq!(pg.stop_kind, "cursor");
    config.operations.insert(
        "GetAllWidgets".into(),
        PaginationSettings {
            has_more_path: Some("/more".into()),
            ..Default::default()
        },
    );
    assert_eq!(
        resolve_pagination(&ir, &config).unwrap()["operation.GetAllWidgets"].stop_kind,
        "has_more"
    );
}

#[test]
fn validates_settings_even_when_no_operation_matches() {
    for change in [
        json!({"default_page_size":0}),
        json!({"max_page_size":50}),
        json!({"start_page":-1}),
        json!({"items_path":"items"}),
        json!({"items_path":"/~2"}),
        json!({"page_param":"header.page"}),
        json!({"page_param":"query.limit"}),
        json!({"has_more_path":"/more"}),
        json!({"start_offset":0}),
    ] {
        let (_, config) = fixture();
        let mut value = serde_json::to_value(config).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .extend(change.as_object().unwrap().clone());
        let config = serde_json::from_value(value).unwrap();
        assert!(
            resolve_pagination(&CoreIr::default(), &config).is_err(),
            "{change}"
        );
    }
}

#[test]
fn explicit_operations_are_strict_but_defaults_skip_incompatible_routes() {
    let (mut ir, mut config) = fixture();
    ir.operations[0].params.clear();
    assert!(resolve_pagination(&ir, &config).unwrap().is_empty());
    config
        .operations
        .insert("GetAllWidgets".into(), Default::default());
    assert!(resolve_pagination(&ir, &config).is_err());
    config.operations.clear();
    config.operations.insert(
        "Typo".into(),
        PaginationSettings {
            disabled: Some(true),
            ..Default::default()
        },
    );
    assert!(resolve_pagination(&ir, &config).is_err());
}

#[test]
fn validates_schema_paths_and_metadata_types() {
    let (ir, config) = fixture();
    for override_settings in [
        PaginationSettings {
            items_path: Some("/missing".into()),
            ..Default::default()
        },
        PaginationSettings {
            items_path: Some("/pages".into()),
            ..Default::default()
        },
        PaginationSettings {
            total_pages_path: Some("/more".into()),
            ..Default::default()
        },
    ] {
        let mut config = config.clone();
        config
            .operations
            .insert("GetAllWidgets".into(), override_settings);
        assert!(resolve_pagination(&ir, &config).is_err());
    }
}

#[test]
fn json_pointer_handles_root_arrays_nested_fields_and_escapes() {
    assert_eq!(
        pointer_tokens("/a~1b/~0key/").unwrap(),
        vec!["a/b", "~key", ""]
    );
    let (mut ir, mut config) = fixture();
    ir.operations[0].responses[0].type_ref = Some(TypeRef::array(TypeRef::primitive("integer")));
    config.defaults.items_path = Some("".into());
    config.defaults.total_pages_path = None;
    config.defaults.stop_on = Some(PaginationStop::EmptyPage);
    assert_eq!(
        resolve_pagination(&ir, &config).unwrap()["operation.GetAllWidgets"].item_type,
        TypeRef::primitive("integer")
    );
    let mut nested = Model::new("nested", "Nested");
    nested
        .fields
        .push(Field::new("a/b", TypeRef::named("Page")));
    ir.models.push(nested);
    ir.operations[0].responses[0].type_ref = Some(TypeRef::named("Nested"));
    config.defaults.items_path = Some("/a~1b/items".into());
    assert_eq!(
        resolve_pagination(&ir, &config).unwrap()["operation.GetAllWidgets"].items_tokens,
        vec!["a/b", "items"]
    );
}
