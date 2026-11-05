use super::*;
use arvalez_ir::{Field, Operation, RequestBody, Response, TupleRest};
use serde_json::json;

fn field(name: &str, ty: TypeRef, optional: bool, nullable: bool) -> Field {
    Field {
        name: name.into(),
        type_ref: ty,
        optional,
        nullable,
        attributes: Default::default(),
    }
}
fn model(name: &str, fields: Vec<Field>) -> Model {
    let mut m = Model::new(name, name);
    m.fields = fields;
    m
}
fn operation(input: Option<&str>, output: &str) -> Operation {
    Operation {
        id: "op".into(),
        name: "op".into(),
        path: "/op".into(),
        request_body: input.map(|name| RequestBody {
            required: true,
            media_type: "application/json".into(),
            type_ref: Some(TypeRef::named(name)),
            attributes: Default::default(),
        }),
        responses: vec![Response {
            status: "default".into(),
            media_type: Some("application/json".into()),
            type_ref: Some(TypeRef::named(output)),
            attributes: Default::default(),
        }],
        ..Default::default()
    }
}
fn all() -> ModelPolicies {
    ModelPolicies {
        response_fields: Some(ResponseFields::AlwaysPresent),
        ..Default::default()
    }
}
fn project(ir: &CoreIr, policies: &ModelPolicies) -> CoreIr {
    prepare_response_models(ir, policies, ModelTarget::Go, crate::to_pascal_case).unwrap()
}
fn get<'a>(ir: &'a CoreIr, name: &str) -> &'a Model {
    ir.models.iter().find(|m| m.name == name).unwrap()
}

#[test]
fn precedence_and_explicit_opt_out() {
    let mut common = all();
    common.overrides.insert(
        "Special".into(),
        ModelOverride {
            response_fields: Some(ResponseFields::Schema),
        },
    );
    let target = all();
    let merged = ModelPolicies::resolve(&common, &target);
    assert_eq!(merged.for_model("Special"), ResponseFields::Schema);
    assert_eq!(merged.for_model("Other"), ResponseFields::AlwaysPresent);
    let mut target = ModelPolicies {
        response_fields: Some(ResponseFields::Schema),
        ..Default::default()
    };
    target.overrides.insert(
        "Special".into(),
        ModelOverride {
            response_fields: Some(ResponseFields::AlwaysPresent),
        },
    );
    let merged = ModelPolicies::resolve(&common, &target);
    assert_eq!(merged.for_model("Special"), ResponseFields::AlwaysPresent);
    assert_eq!(merged.for_model("Other"), ResponseFields::Schema);
    assert_eq!(
        ModelPolicies::default().for_model("Other"),
        ResponseFields::Schema
    );
    assert!(serde_json::from_value::<ModelPolicies>(json!({"response_fields":"native"})).is_err());
    assert!(serde_json::from_value::<ModelPolicies>(json!({"typo":true})).is_err());
}

#[test]
fn splits_shared_parents_but_only_changes_each_models_own_fields() {
    let child = model(
        "Child",
        vec![field("value", TypeRef::primitive("string"), true, true)],
    );
    let parent = model(
        "Parent",
        vec![field("child", TypeRef::named("Child"), true, false)],
    );
    let ir = CoreIr {
        models: vec![parent, child],
        operations: vec![operation(Some("Parent"), "Parent")],
        ..Default::default()
    };
    let original = ir.clone();
    let mut policies = all();
    policies.overrides.insert(
        "Parent".into(),
        ModelOverride {
            response_fields: Some(ResponseFields::Schema),
        },
    );
    let result = project(&ir, &policies);
    assert_eq!(ir, original);
    assert_eq!(get(&result, "Parent"), get(&ir, "Parent"));
    assert_eq!(get(&result, "Child"), get(&ir, "Child"));
    assert!(get(&result, "ParentResponse").fields[0].optional);
    assert_eq!(
        get(&result, "ParentResponse").fields[0].type_ref,
        TypeRef::named("ChildResponse")
    );
    assert!(!get(&result, "ChildResponse").fields[0].optional);
    assert!(get(&result, "ChildResponse").fields[0].nullable);
    assert_eq!(
        result.operations[0].request_body,
        ir.operations[0].request_body
    );
    assert_eq!(
        result.operations[0].responses[0].type_ref,
        Some(TypeRef::named("ParentResponse"))
    );
    let mut only_parent = ModelPolicies::default();
    only_parent.overrides.insert(
        "Parent".into(),
        ModelOverride {
            response_fields: Some(ResponseFields::AlwaysPresent),
        },
    );
    let result = project(&ir, &only_parent);
    assert!(!get(&result, "ParentResponse").fields[0].optional);
    assert_eq!(
        get(&result, "ParentResponse").fields[0].type_ref,
        TypeRef::named("Child")
    );
    assert!(!result.models.iter().any(|m| m.name == "ChildResponse"));
}

#[test]
fn response_exclusive_models_keep_names_and_unused_references_protect_originals() {
    let child = model(
        "Child",
        vec![field("n", TypeRef::primitive("integer"), true, false)],
    );
    let mut ir = CoreIr {
        models: vec![child],
        operations: vec![operation(None, "Child")],
        ..Default::default()
    };
    let result = project(&ir, &all());
    assert_eq!(result.models.len(), 1);
    assert!(!get(&result, "Child").fields[0].optional);
    ir.models.push(model(
        "Unused",
        vec![field("child", TypeRef::named("Child"), true, false)],
    ));
    let result = project(&ir, &all());
    assert_eq!(get(&result, "Unused"), get(&ir, "Unused"));
    assert_eq!(get(&result, "Child"), get(&ir, "Child"));
    assert!(!get(&result, "ChildResponse").fields[0].optional);
}

#[test]
fn recursive_projection_is_finite_and_names_are_stable() {
    let node = model(
        "Node",
        vec![field("next", TypeRef::named("Node"), true, false)],
    );
    let collision = model("NodeResponse", vec![]);
    let ir = CoreIr {
        models: vec![node, collision],
        operations: vec![operation(Some("Node"), "Node")],
        ..Default::default()
    };
    let result = project(&ir, &all());
    let node = get(&result, "NodeResponse2");
    assert_eq!(node.fields[0].type_ref, TypeRef::named("NodeResponse2"));
    assert_eq!(node.fields[0].attributes[RECURSIVE_POINTER], json!(true));
    let mut reversed = ir.clone();
    reversed.models.reverse();
    assert_eq!(get(&project(&reversed, &all()), "NodeResponse2"), node);
    arvalez_ir::validate_ir(&result).unwrap();
}

#[test]
fn traverses_aliases_unions_maps_and_tuples() {
    let leaf = model(
        "Leaf",
        vec![field("n", TypeRef::primitive("string"), true, false)],
    );
    let nested = TypeRef::Union {
        variants: vec![
            TypeRef::map(TypeRef::array(TypeRef::named("Leaf"))),
            TypeRef::Tuple {
                items: vec![TypeRef::named("Leaf")],
                rest: TupleRest::Typed {
                    item: Box::new(TypeRef::named("Leaf")),
                },
            },
        ],
    };
    let mut alias = model("Alias", vec![]);
    alias.attributes.insert(
        "alias_type_ref".into(),
        serde_json::to_value(nested).unwrap(),
    );
    let ir = CoreIr {
        models: vec![alias, leaf],
        operations: vec![operation(Some("Alias"), "Alias")],
        ..Default::default()
    };
    let result = project(&ir, &all());
    let alias = get(&result, "AliasResponse");
    let mut refs = BTreeSet::new();
    references(
        &serde_json::from_value(alias.attributes["alias_type_ref"].clone()).unwrap(),
        &mut refs,
    );
    assert_eq!(refs, BTreeSet::from(["LeafResponse".into()]));
}

#[test]
fn schema_is_identity_and_unknown_override_is_rejected() {
    let ir = CoreIr {
        models: vec![model(
            "User",
            vec![field("x", TypeRef::primitive("string"), true, true)],
        )],
        operations: vec![operation(Some("User"), "User")],
        ..Default::default()
    };
    assert_eq!(project(&ir, &ModelPolicies::default()), ir);
    let mut policies = all();
    policies
        .overrides
        .insert("Typo".into(), ModelOverride::default());
    assert!(
        prepare_response_models(&ir, &policies, ModelTarget::Go, crate::to_pascal_case)
            .unwrap_err()
            .to_string()
            .contains("Typo")
    );
}

#[test]
fn already_required_nullable_fields_only_split_when_representation_changes() {
    let ir = CoreIr {
        models: vec![model(
            "User",
            vec![field("x", TypeRef::primitive("string"), false, true)],
        )],
        operations: vec![operation(Some("User"), "User")],
        ..Default::default()
    };
    assert_eq!(project(&ir, &all()).models.len(), 2);
    let ts = prepare_response_models(&ir, &all(), ModelTarget::TypeScript, crate::to_pascal_case)
        .unwrap();
    assert_eq!(ts.models.len(), 1);
    let mut python = ir.clone();
    python.models[0].fields[0]
        .attributes
        .insert("default".into(), json!(null));
    let py = prepare_response_models(&python, &all(), ModelTarget::Python, crate::to_pascal_case)
        .unwrap();
    assert_eq!(py.models.len(), 2);
    assert_eq!(
        get(&py, "UserResponse").fields[0].attributes["default"],
        json!(null)
    );
}

#[test]
fn deprecated_filtering_does_not_change_response_shapes() {
    let mut ir = CoreIr {
        models: vec![model(
            "User",
            vec![field("x", TypeRef::primitive("string"), true, true)],
        )],
        operations: vec![operation(Some("User"), "User")],
        ..Default::default()
    };
    ir.operations[0]
        .attributes
        .insert("deprecated".into(), json!(true));
    let common = crate::CommonConfig {
        models: all(),
        skip_deprecated_operations: true,
        ..Default::default()
    };
    let filtered =
        prepare_generation_ir(&ir, &common, ModelTarget::Go, crate::to_pascal_case).unwrap();
    assert!(filtered.operations.is_empty());
    assert_eq!(filtered.models, project(&ir, &all()).models);
}
