use super::*;
use arvalez_ir::{ModelKind, TupleRest};
use std::{fs, process::Command};
use tempfile::tempdir;

fn field(name: &str, ty: TypeRef, optional: bool, nullable: bool) -> Field {
    Field {
        name: name.into(),
        type_ref: ty,
        optional,
        nullable,
        attributes: Default::default(),
    }
}
fn fixture() -> (CoreIr, TargetConfig, CommonConfig) {
    let mut child = Model::new("child", "Child");
    child.fields = vec![
        field("action", TypeRef::primitive("string"), false, false),
        field("score", TypeRef::primitive("integer"), true, false),
    ];
    let mut status = Model::new("status", "Status");
    status.kind = ModelKind::Enum {
        base: TypeRef::primitive("string"),
        values: vec![json!("ready")],
    };
    let mut widget = Model::new("widget", "Widget");
    widget.fields = vec![
        field("action", TypeRef::primitive("string"), false, false),
        field("title", TypeRef::primitive("string"), true, false),
        field("count", TypeRef::primitive("integer"), true, false),
        field("enabled", TypeRef::primitive("boolean"), false, false),
        field("ratio", TypeRef::primitive("number"), false, false),
        field("status", TypeRef::named("Status"), true, false),
        field("child", TypeRef::named("Child"), true, false),
        field(
            "children",
            TypeRef::Array {
                item: Box::new(TypeRef::named("Child")),
            },
            true,
            false,
        ),
        field(
            "labels",
            TypeRef::Map {
                value: Box::new(TypeRef::primitive("string")),
            },
            true,
            false,
        ),
        field(
            "groups",
            TypeRef::Map {
                value: Box::new(TypeRef::named("Child")),
            },
            true,
            false,
        ),
        field(
            "matrix",
            TypeRef::Array {
                item: Box::new(TypeRef::Array {
                    item: Box::new(TypeRef::primitive("integer")),
                }),
            },
            true,
            false,
        ),
        field("blob", TypeRef::primitive("binary"), true, false),
        field("nullable", TypeRef::primitive("string"), true, true),
        field("clear", TypeRef::primitive("integer"), true, true),
        field(
            "required_nullable",
            TypeRef::primitive("string"),
            false,
            true,
        ),
        field("payload", TypeRef::primitive("any"), true, false),
        field("next", TypeRef::named("Widget"), true, false),
        field(
            "tuple",
            TypeRef::Tuple {
                items: vec![TypeRef::primitive("string"), TypeRef::primitive("integer")],
                rest: TupleRest::Forbidden,
            },
            false,
            false,
        ),
    ];
    child.fields[0].attributes.insert(
        "description".into(),
        json!("Action description \"quoted\"\nnext line"),
    );
    let mut ir = CoreIr {
        models: vec![widget, status, child],
        ..Default::default()
    };
    let mut cycle_a = Model::new("cycle_a", "CycleA");
    cycle_a.fields = vec![
        field("label", TypeRef::primitive("string"), false, false),
        field("b", TypeRef::named("CycleB"), true, false),
        field(
            "children",
            TypeRef::Array {
                item: Box::new(TypeRef::named("CycleA")),
            },
            true,
            false,
        ),
    ];
    let mut cycle_b = Model::new("cycle_b", "CycleB");
    cycle_b.fields = vec![
        field("a", TypeRef::named("CycleA"), true, false),
        field(
            "by_name",
            TypeRef::Map {
                value: Box::new(TypeRef::named("CycleB")),
            },
            true,
            false,
        ),
    ];
    let mut envelope = Model::new("envelope", "Envelope");
    envelope.fields = vec![
        field("node", TypeRef::named("CycleA"), true, false),
        field(
            "choices",
            TypeRef::Array {
                item: Box::new(TypeRef::Union {
                    variants: vec![TypeRef::primitive("string"), TypeRef::primitive("integer")],
                }),
            },
            true,
            false,
        ),
        field(
            "extras",
            TypeRef::Map {
                value: Box::new(TypeRef::primitive("any")),
            },
            true,
            false,
        ),
        field(
            "choice",
            TypeRef::Union {
                variants: vec![TypeRef::primitive("string"), TypeRef::primitive("integer")],
            },
            false,
            false,
        ),
    ];
    ir.models.extend([cycle_a, cycle_b, envelope]);
    let fixture_dir = tempdir().unwrap();
    let fixture_path = fixture_dir.path().join("nullable-3.0.json");
    fs::write(&fixture_path, json!({
        "openapi": "3.0.3", "paths": {}, "components": {"schemas": {
            "NullableInput": {"type": "object", "required": ["requiredValue", "plainValue"], "properties": {
                "requiredValue": {"type": "string", "nullable": true},
                "optionalValue": {"type": "string", "nullable": true},
                "explicitValue": {"type": "integer", "nullable": true},
                "plainValue": {"type": "boolean"},
                "numbers": {"type": "array", "items": {"type": "integer"}, "nullable": true},
                "labels": {"type": "object", "additionalProperties": {"type": "string"}, "nullable": true},
                "child": {"$ref": "#/components/schemas/NullableChild"},
                "state": {"$ref": "#/components/schemas/NullableState"}
            }},
            "NullableChild": {"type": "object", "nullable": true, "required": ["action"], "properties": {"action": {"type": "string"}}},
            "NullableState": {"type": "string", "nullable": true, "enum": ["ready", "done"]}
        }}
    }).to_string()).unwrap();
    ir.models.extend(
        arvalez_openapi::load_openapi_to_ir(&fixture_path)
            .unwrap()
            .models,
    );
    let mut config = TargetConfig {
        module_path: "example.com/tfmodels".into(),
        sdk_module_path: "example.com/sdk/v2".into(),
        sdk_version: "v2.0.0".into(),
        ..Default::default()
    };
    let mut options = ModelOptions::default();
    options.fields.insert(
        "clear".into(),
        FieldOptions {
            null_behavior: NullBehavior::ExplicitNull,
            ..Default::default()
        },
    );
    config.models.insert("Widget".into(), options);
    let mut nullable_options = ModelOptions::default();
    nullable_options.fields.insert(
        "explicitValue".into(),
        FieldOptions {
            null_behavior: NullBehavior::ExplicitNull,
            ..Default::default()
        },
    );
    config
        .models
        .insert("NullableInput".into(), nullable_options);
    let common = CommonConfig {
        package: arvalez_target_core::PackageConfig {
            name: "tfmodels".into(),
            ..Default::default()
        },
    };
    (ir, config, common)
}
fn source(files: &[GeneratedFile], name: &str) -> String {
    files
        .iter()
        .find(|f| f.path == Path::new(name))
        .unwrap()
        .contents
        .clone()
}
#[test]
fn deterministic_typed_output_and_template_overrides() {
    let (mut ir, config, common) = fixture();
    let first = generate(&ir, None, &common, &config).unwrap();
    ir.models.reverse();
    let second = generate(&ir, None, &common, &config).unwrap();
    assert_eq!(
        serde_json::to_value(&first).unwrap(),
        serde_json::to_value(&second).unwrap()
    );
    assert!(source(&first, "adapters.go").contains("value.Action"));
    assert!(source(&first, "adapters.go").contains("sdk.Widget"));
    assert!(source(&first, "models.go").contains("`tfsdk:\"action\"`"));
    let dir = tempdir().unwrap();
    dump_templates(dir.path()).unwrap();
    fs::write(
        dir.path().join("root/README.md.tera"),
        "custom {{ package_name }}",
    )
    .unwrap();
    fs::write(dir.path().join("root/~schema.go.tera"), "").unwrap();
    let files = generate(&ir, Some(dir.path()), &common, &config).unwrap();
    assert_eq!(source(&files, "README.md"), "custom tfmodels");
    assert!(!files.iter().any(|f| f.path == Path::new("schema.go")));
}
#[test]
fn current_sdk_version_uses_package_version_and_renders_a_pin() {
    let (ir, mut config, mut common) = fixture();
    config.sdk_version = TargetConfig::default().sdk_version;
    assert_eq!(config.sdk_version, "current");
    let parsed: TargetConfig = serde_json::from_value(json!({
        "module_path": config.module_path, "sdk_module_path": config.sdk_module_path
    }))
    .unwrap();
    assert_eq!(parsed.sdk_version, "current");
    for version in ["2.4.0", "v2.4.0", "2.4.0-rc.1"] {
        common.package.version = version.into();
        let files = generate(&ir, None, &common, &config).unwrap();
        let expected = format!("example.com/sdk/v2 v{}", version.trim_start_matches('v'));
        assert!(source(&files, "go.mod").contains(&expected));
        assert!(!source(&files, "go.mod").contains("current"));
    }
    common.package.version = "latest".into();
    assert!(generate(&ir, None, &common, &config).is_err());
    config.sdk_version = "v2.0.0".into();
    assert!(
        source(&generate(&ir, None, &common, &config).unwrap(), "go.mod")
            .contains("example.com/sdk/v2 v2.0.0")
    );
}

#[test]
fn automatic_json_matches_explicit_overrides_and_keeps_native_fields() {
    let (ir, config, common) = fixture();
    let automatic = generate(&ir, None, &common, &config).unwrap();
    let mut explicit = config.clone();
    for key in ["payload", "next", "tuple"] {
        explicit.models.get_mut("Widget").unwrap().fields.insert(
            key.into(),
            FieldOptions {
                representation: Some(Representation::Json),
                ..Default::default()
            },
        );
    }
    let explicit_files = generate(&ir, None, &common, &explicit).unwrap();
    for file in ["models.go", "adapters.go", "schema.go"] {
        assert_eq!(source(&automatic, file), source(&explicit_files, file));
    }
    let resolver = Resolver::new(&ir, &config);
    assert!(
        resolver
            .json_fields
            .contains_key(&("CycleA".into(), "b".into()))
    );
    assert!(
        resolver
            .json_fields
            .contains_key(&("CycleB".into(), "a".into()))
    );
    assert!(
        !resolver
            .json_fields
            .contains_key(&("Envelope".into(), "node".into()))
    );
    explicit
        .models
        .entry("CycleA".into())
        .or_default()
        .fields
        .insert(
            "b".into(),
            FieldOptions {
                representation: Some(Representation::Json),
                ..Default::default()
            },
        );
    let resolver = Resolver::new(&ir, &explicit);
    assert!(
        !resolver
            .json_fields
            .contains_key(&("CycleB".into(), "a".into()))
    );
    explicit.models.get_mut("Widget").unwrap().fields.insert(
        "child".into(),
        FieldOptions {
            representation: Some(Representation::Json),
            ..Default::default()
        },
    );
    let files = generate(&ir, None, &common, &explicit).unwrap();
    assert!(source(&files, "adapters.go").contains("jsonCodec[sdk.Child]()"));
    assert!(source(&automatic, "README.md").contains("`CycleA.b`"));
}

#[test]
fn rejects_invalid_configuration_and_unknown_models() {
    let (ir, config, common) = fixture();
    let mut changed = config.clone();
    changed.sdk_version.clear();
    assert!(generate(&ir, None, &common, &changed).is_err());
    for version in ["vfoo", "latest", "v01.2.3", "v1.2", "v1.2.3-"] {
        let mut changed = config.clone();
        changed.sdk_version = version.into();
        assert!(generate(&ir, None, &common, &changed).is_err());
    }

    let mut changed = config.clone();
    changed.sdk_import_path = Some("unrelated.com/sdk".into());
    assert!(generate(&ir, None, &common, &changed).is_err());
    let mut changed = config.clone();
    changed
        .models
        .insert("Missing".into(), ModelOptions::default());
    assert!(generate(&ir, None, &common, &changed).is_err());
    let mut changed = config.clone();
    changed
        .models
        .get_mut("Widget")
        .unwrap()
        .fields
        .insert("missing".into(), FieldOptions::default());
    assert!(generate(&ir, None, &common, &changed).is_err());
    let mut changed = config.clone();
    changed.models.get_mut("Widget").unwrap().fields.insert(
        "title".into(),
        FieldOptions {
            null_behavior: NullBehavior::ExplicitNull,
            ..Default::default()
        },
    );
    assert!(generate(&ir, None, &common, &changed).is_err());
    let mut changed = ir.clone();
    changed.models[0].fields[0].type_ref = TypeRef::named("Missing");
    assert!(generate(&changed, None, &common, &config).is_err());
}
#[test]
fn validates_attribute_and_sdk_collisions() {
    let (mut ir, mut config, common) = fixture();
    config.models.get_mut("Widget").unwrap().fields.insert(
        "title".into(),
        FieldOptions {
            name: Some("action".into()),
            ..Default::default()
        },
    );
    assert!(
        generate(&ir, None, &common, &config)
            .unwrap_err()
            .to_string()
            .contains("colliding Terraform")
    );
    config
        .models
        .get_mut("Widget")
        .unwrap()
        .fields
        .remove("title");
    ir.models[0]
        .fields
        .push(field("a-b", TypeRef::primitive("string"), true, false));
    ir.models[0]
        .fields
        .push(field("a_b", TypeRef::primitive("string"), true, false));
    config.models.get_mut("Widget").unwrap().fields.insert(
        "a_b".into(),
        FieldOptions {
            name: Some("other".into()),
            ..Default::default()
        },
    );
    assert!(
        generate(&ir, None, &common, &config)
            .unwrap_err()
            .to_string()
            .contains("SDK field")
    );
}

#[test]
fn generated_sdk_and_terraform_module_pass_framework_integration() {
    if Command::new("go").arg("version").output().is_err() {
        assert!(
            std::env::var_os("ARVALEZ_REQUIRE_GO").is_none(),
            "Go is required by this CI job"
        );
        eprintln!("Go unavailable; set ARVALEZ_REQUIRE_GO=1 to require integration tests");
        return;
    }
    let (ir, config, common) = fixture();
    let dir = tempdir().unwrap();
    let sdk_dir = dir.path().join("sdk");
    let adapter_dir = dir.path().join("tfmodels");
    let sdk_common = CommonConfig {
        package: arvalez_target_core::PackageConfig {
            name: "sdk".into(),
            ..Default::default()
        },
    };
    let sdk_files = arvalez_target_go::generate_go_package(
        &ir,
        None,
        &sdk_common,
        &arvalez_target_go::TargetConfig {
            module_path: config.sdk_module_path.clone(),
            ..Default::default()
        },
    )
    .unwrap();
    arvalez_target_go::write_go_package(&sdk_dir, &sdk_files).unwrap();
    write_package(
        &adapter_dir,
        &generate(&ir, None, &common, &config).unwrap(),
    )
    .unwrap();
    fs::write(
        adapter_dir.join("integration_test.go"),
        include_str!("integration_test.go"),
    )
    .unwrap();
    let modfile = adapter_dir.join("go.mod");
    let mut contents = fs::read_to_string(&modfile).unwrap();
    contents.push_str("\nreplace example.com/sdk/v2 => ../sdk\n");
    fs::write(modfile, contents).unwrap();
    for args in [&["mod", "tidy"][..], &["test", "-v", "./..."][..]] {
        let output = Command::new("go")
            .args(args)
            .current_dir(&adapter_dir)
            .env("GOWORK", "off")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "go {args:?}:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    for (name, models) in [
        ("empty", vec![]),
        ("object", vec![Model::new("empty", "Empty")]),
        (
            "enum",
            vec![
                ir.models
                    .iter()
                    .find(|m| m.name == "Status")
                    .unwrap()
                    .clone(),
            ],
        ),
    ] {
        let minimal_ir = CoreIr {
            models,
            ..Default::default()
        };
        let mut minimal_config = config.clone();
        minimal_config.models.clear();
        let sdk_files = arvalez_target_go::generate_go_package(
            &minimal_ir,
            None,
            &sdk_common,
            &arvalez_target_go::TargetConfig {
                module_path: config.sdk_module_path.clone(),
                ..Default::default()
            },
        )
        .unwrap();
        arvalez_target_go::write_go_package(&sdk_dir, &sdk_files).unwrap();
        let output = dir.path().join(name);
        write_package(
            &output,
            &generate(&minimal_ir, None, &common, &minimal_config).unwrap(),
        )
        .unwrap();
        let modfile = output.join("go.mod");
        fs::write(
            &modfile,
            format!(
                "{}\nreplace example.com/sdk/v2 => ../sdk\n",
                fs::read_to_string(&modfile).unwrap()
            ),
        )
        .unwrap();
        for args in [&["mod", "tidy"][..], &["test", "./..."][..]] {
            let result = Command::new("go")
                .args(args)
                .current_dir(&output)
                .env("GOWORK", "off")
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{name}: {}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        }
    }
}
